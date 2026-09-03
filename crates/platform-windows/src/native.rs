//! The only module in the project with direct Win32 FFI.
//!
//! Each unsafe block maps one documented Windows call and keeps pointers local.
//! No borrowed data crosses this boundary.

use std::{
    mem::{size_of, zeroed},
    sync::{Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use free_whisper_domain::TargetWindowSnapshot;
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, GetLastError, HANDLE, HWND},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::{
        DataExchange::{
            CloseClipboard, EnumClipboardFormats, GetClipboardSequenceNumber, OpenClipboard,
        },
        Ole::CF_UNICODETEXT,
        Threading::{
            GetCurrentThreadId, GetProcessTimes, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL,
            MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, SendInput, UnregisterHotKey, VK_CONTROL,
            VK_SPACE, VK_V,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetMessageW, GetWindowTextLengthW, GetWindowTextW,
            GetWindowThreadProcessId, IsWindow, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
            SetForegroundWindow, WM_HOTKEY, WM_QUIT,
        },
    },
};

use crate::{
    ClipboardError, ClipboardRestoreOutcome, ClipboardRestoreTicket, ClipboardWriter,
    ForegroundWindow, HotkeyBinding, HotkeyError, HotkeyEvent, PasteDecision, PasteRefusal,
    SecretStore, SecretStoreError, WindowError, evaluate_paste_policy,
    validate_credential_reference, validate_credential_secret,
};

const HOTKEY_ID: i32 = 0x4657;

/// Explicit clipboard writer. It only writes when the caller asks it to.
#[derive(Debug, Default)]
pub struct WindowsClipboard;

impl ClipboardWriter for WindowsClipboard {
    fn copy_text(&self, text: &str) -> Result<(), ClipboardError> {
        validate_copy_text(text)?;
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))?;
        clipboard
            .set_text(text)
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))
    }
}

impl WindowsClipboard {
    /// Records a ticket only when the former clipboard has exactly the Unicode
    /// text format. Images, files and unknown formats are never guessed.
    pub fn copy_with_restore_ticket(
        &self,
        text: &str,
    ) -> Result<ClipboardRestoreTicket, ClipboardError> {
        validate_copy_text(text)?;
        let original_text = if clipboard_contains_only_unicode_text()? {
            let mut clipboard = arboard::Clipboard::new()
                .map_err(|error| ClipboardError::Unavailable(error.to_string()))?;
            Some(
                clipboard
                    .get_text()
                    .map_err(|error| ClipboardError::Unavailable(error.to_string()))?,
            )
        } else {
            None
        };
        self.copy_text(text)?;
        Ok(ClipboardRestoreTicket {
            expected_sequence: clipboard_sequence_number(),
            original_text,
        })
    }

    #[must_use]
    pub fn restore_if_unchanged(&self, ticket: ClipboardRestoreTicket) -> ClipboardRestoreOutcome {
        let Some(original_text) = ticket.original_text else {
            return ClipboardRestoreOutcome::NotRestorable;
        };
        if clipboard_sequence_number() != ticket.expected_sequence {
            return ClipboardRestoreOutcome::SkippedSequenceChanged;
        }
        match self.copy_text(&original_text) {
            Ok(()) => ClipboardRestoreOutcome::Restored,
            Err(error) => ClipboardRestoreOutcome::Failed(error.to_string()),
        }
    }
}

/// A user-scoped generic Credential Manager entry. Only opaque references in
/// the `free-whisper/provider/` namespace are accepted, preventing accidental
/// access to credentials owned by other applications.
#[derive(Debug, Default)]
pub struct WindowsCredentialStore;

impl SecretStore for WindowsCredentialStore {
    fn write_secret(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError> {
        validate_credential_reference(reference)?;
        validate_credential_secret(secret)?;
        let mut target = wide_nul(reference);
        let mut username = wide_nul("free-whisper");
        let mut blob = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: unsafe { zeroed() },
            CredentialBlobSize: u32::try_from(blob.len())
                .map_err(|_| SecretStoreError::InvalidSecret)?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: username.as_mut_ptr(),
        };
        // SAFETY: every pointer in CREDENTIALW targets local, nul-terminated or
        // byte storage that stays live for the documented synchronous call.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(SecretStoreError::Unavailable(last_error_message()));
        }
        Ok(())
    }

    fn read_secret(&self, reference: &str) -> Result<String, SecretStoreError> {
        validate_credential_reference(reference)?;
        let target = wide_nul(reference);
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: target is a valid nul-terminated UTF-16 string and credential
        // is writable storage for the API-owned result pointer.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            return Err(secret_store_last_error());
        }
        struct CredentialAllocation(*mut CREDENTIALW);
        impl Drop for CredentialAllocation {
            fn drop(&mut self) {
                // SAFETY: CredReadW returned this allocation and it is freed
                // exactly once when this guard leaves scope.
                unsafe { CredFree(self.0.cast()) };
            }
        }
        let allocation = CredentialAllocation(credential);
        // SAFETY: successful CredReadW guarantees a valid CREDENTIALW and a
        // credential blob of the reported length until CredFree is called.
        let secret = unsafe {
            let bytes = std::slice::from_raw_parts(
                (*allocation.0).CredentialBlob.cast_const(),
                (*allocation.0).CredentialBlobSize as usize,
            );
            std::str::from_utf8(bytes)
                .map_err(|_| SecretStoreError::Unavailable("credential is not UTF-8".to_owned()))?
                .to_owned()
        };
        validate_credential_secret(&secret)?;
        Ok(secret)
    }

    fn delete_secret(&self, reference: &str) -> Result<(), SecretStoreError> {
        validate_credential_reference(reference)?;
        let target = wide_nul(reference);
        // SAFETY: target is a valid nul-terminated UTF-16 string and this API
        // does not retain the pointer after it returns.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let error = secret_store_last_error();
            if matches!(error, SecretStoreError::NotFound) {
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }
}

/// Owns the isolated WM_HOTKEY loop and always unregisters before shutdown.
pub struct GlobalHotkey {
    thread_id: u32,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl GlobalHotkey {
    pub fn start(
        binding: HotkeyBinding,
        events: mpsc::Sender<HotkeyEvent>,
    ) -> Result<Self, HotkeyError> {
        binding.validate()?;
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let join_handle = thread::Builder::new()
            .name("free-whisper-hotkey".to_owned())
            .spawn(move || hotkey_message_loop(binding, events, started_tx))
            .map_err(|error| HotkeyError::StartupFailed(error.to_string()))?;
        let thread_id = started_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| HotkeyError::StartupFailed(error.to_string()))??;
        Ok(Self {
            thread_id,
            join_handle: Mutex::new(Some(join_handle)),
        })
    }

    pub fn shutdown(&self) {
        // SAFETY: thread_id came from our message-loop thread; this posts only
        // the documented WM_QUIT termination request to its private queue.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Ok(mut handle) = self.join_handle.lock()
            && let Some(handle) = handle.take()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for GlobalHotkey {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Debug, Default)]
pub struct WindowsDesktop;

impl WindowsDesktop {
    pub fn capture_foreground_window(&self) -> Result<Option<ForegroundWindow>, WindowError> {
        // SAFETY: no pointers; Windows returns a nullable foreground HWND.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return Ok(None);
        }
        snapshot_for_handle(hwnd).map(Some)
    }

    pub fn capture_target_snapshot(&self) -> Result<Option<TargetWindowSnapshot>, WindowError> {
        self.capture_foreground_window()
            .map(|foreground| foreground.map(|value| value.snapshot))
    }

    /// For a confirmation click, requests focus first then verifies the exact
    /// target again. Automatic paste calls this with `false`.
    pub fn paste_into_original(
        &self,
        snapshot: &TargetWindowSnapshot,
        confirmed_by_user: bool,
    ) -> Result<(), PasteRefusalOrError> {
        if confirmed_by_user {
            // SAFETY: this HWND is revalidated against the foreground snapshot
            // immediately after focus is requested.
            if unsafe { SetForegroundWindow(snapshot.window_handle as HWND) } == 0 {
                return Err(PasteRefusalOrError::Refused(PasteRefusal::FocusDenied));
            }
            thread::sleep(Duration::from_millis(60));
        }
        let foreground = self
            .capture_foreground_window()
            .map_err(PasteRefusalOrError::Error)?;
        match evaluate_paste_policy(Some(snapshot), foreground.as_ref()) {
            PasteDecision::Allowed => send_control_v().map_err(PasteRefusalOrError::Error),
            PasteDecision::Refused(refusal) => Err(PasteRefusalOrError::Refused(refusal)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PasteRefusalOrError {
    #[error("paste refused: {0:?}")]
    Refused(PasteRefusal),
    #[error(transparent)]
    Error(#[from] WindowError),
}

fn hotkey_message_loop(
    binding: HotkeyBinding,
    events: mpsc::Sender<HotkeyEvent>,
    started: mpsc::SyncSender<Result<u32, HotkeyError>>,
) {
    // SAFETY: a zeroed MSG is valid initial storage; PeekMessageW creates this
    // thread's queue and does not retain the buffer.
    unsafe {
        let mut message: MSG = zeroed();
        let _ = PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    // SAFETY: null HWND requests a thread-global binding; modifiers and key are
    // safe fixed values checked by HotkeyBinding.
    let registered = unsafe {
        RegisterHotKey(
            std::ptr::null_mut(),
            HOTKEY_ID,
            hotkey_modifiers(&binding),
            VK_SPACE as u32,
        )
    };
    if registered == 0 {
        let _ = started.send(Err(HotkeyError::RegistrationFailed(last_error_message())));
        return;
    }
    // SAFETY: retrieves this calling message thread's id.
    let thread_id = unsafe { GetCurrentThreadId() };
    if started.send(Ok(thread_id)).is_err() {
        // SAFETY: balances the successful registration above.
        unsafe {
            let _ = UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
        }
        return;
    }
    loop {
        // SAFETY: pointer refers to valid MSG memory for the duration of call.
        let mut message: MSG = unsafe { zeroed() };
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if message.message == WM_HOTKEY
            && message.wParam == HOTKEY_ID as usize
            && events.send(HotkeyEvent::Pressed).is_err()
        {
            break;
        }
    }
    // SAFETY: balances successful RegisterHotKey on all loop exits.
    unsafe {
        let _ = UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
    }
}

fn hotkey_modifiers(binding: &HotkeyBinding) -> u32 {
    let mut modifiers = MOD_NOREPEAT;
    if binding.control {
        modifiers |= MOD_CONTROL;
    }
    if binding.alt {
        modifiers |= MOD_ALT;
    }
    if binding.shift {
        modifiers |= MOD_SHIFT;
    }
    modifiers
}

fn snapshot_for_handle(hwnd: HWND) -> Result<ForegroundWindow, WindowError> {
    // SAFETY: Windows validates the opaque HWND; no pointer is passed.
    if unsafe { IsWindow(hwnd) } == 0 {
        return Err(WindowError::Inspection(
            "the foreground window no longer exists".to_owned(),
        ));
    }
    let mut process_id = 0;
    // SAFETY: process_id is valid writable memory for the output parameter.
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, &mut process_id);
    }
    if process_id == 0 {
        return Err(WindowError::Inspection(
            "the foreground window has no process id".to_owned(),
        ));
    }
    let (process_started_at_filetime, elevated) = process_details(process_id)?;
    Ok(ForegroundWindow {
        snapshot: TargetWindowSnapshot {
            window_handle: hwnd as u64,
            process_id,
            process_started_at_filetime,
            title: window_title(hwnd),
        },
        elevated,
    })
}

fn process_details(process_id: u32) -> Result<(u64, bool), WindowError> {
    // SAFETY: read-only access; handle is checked then closed exactly once.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Err(WindowError::Inspection(last_error_message()));
    }
    let result = process_details_for_handle(process);
    // SAFETY: process is valid after non-null OpenProcess above.
    unsafe {
        let _ = CloseHandle(process);
    }
    result
}

fn process_details_for_handle(process: HANDLE) -> Result<(u64, bool), WindowError> {
    let mut created: FILETIME = unsafe { zeroed() };
    let mut ignored: FILETIME = unsafe { zeroed() };
    // SAFETY: each FILETIME pointer targets valid writable storage.
    if unsafe {
        GetProcessTimes(
            process,
            &mut created,
            &mut ignored,
            &mut ignored,
            &mut ignored,
        )
    } == 0
    {
        return Err(WindowError::Inspection(last_error_message()));
    }
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: token is valid output storage and process is a live handle.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(WindowError::Inspection(last_error_message()));
    }
    let elevation = token_elevation(token);
    // SAFETY: token is non-null after successful OpenProcessToken.
    unsafe {
        let _ = CloseHandle(token);
    }
    let start = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    elevation.map(|elevated| (start, elevated))
}

fn token_elevation(token: HANDLE) -> Result<bool, WindowError> {
    let mut elevation: TOKEN_ELEVATION = unsafe { zeroed() };
    let mut returned = 0;
    // SAFETY: buffer matches TOKEN_ELEVATION and exists for the entire call.
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    if result == 0 || returned < size_of::<TOKEN_ELEVATION>() as u32 {
        return Err(WindowError::Inspection(last_error_message()));
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn window_title(hwnd: HWND) -> String {
    // SAFETY: no pointers and a zero result is a normal empty title.
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return String::new();
    }
    let mut units = vec![0u16; length as usize + 1];
    // SAFETY: mutable UTF-16 buffer includes room for a terminating nul.
    let copied = unsafe { GetWindowTextW(hwnd, units.as_mut_ptr(), units.len() as i32) };
    String::from_utf16_lossy(&units[..copied.max(0) as usize])
}

fn send_control_v() -> Result<(), WindowError> {
    let inputs = [
        key_input(VK_CONTROL, 0),
        key_input(VK_V, 0),
        key_input(VK_V, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    // SAFETY: inputs is a contiguous C-compatible INPUT array held alive for
    // the full call and the declared element size/count match it exactly.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        return Err(WindowError::Injection(last_error_message()));
    }
    Ok(())
}

fn key_input(key: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn clipboard_contains_only_unicode_text() -> Result<bool, ClipboardError> {
    // SAFETY: null owner is documented; CloseClipboard below balances opening.
    if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
        return Err(ClipboardError::Unavailable(last_error_message()));
    }
    let mut format = 0;
    let mut saw_format = false;
    let mut only_text = true;
    loop {
        // SAFETY: format contains the previous enumerated id, starting at zero.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        saw_format = true;
        if format != u32::from(CF_UNICODETEXT) {
            only_text = false;
            break;
        }
    }
    // SAFETY: pairs this function's successful OpenClipboard.
    unsafe {
        let _ = CloseClipboard();
    }
    Ok(saw_format && only_text)
}

fn clipboard_sequence_number() -> u32 {
    // SAFETY: no pointers; returns Windows' clipboard sequence counter.
    unsafe { GetClipboardSequenceNumber() }
}

fn validate_copy_text(text: &str) -> Result<(), ClipboardError> {
    if text.is_empty() {
        Err(ClipboardError::EmptyText)
    } else {
        Ok(())
    }
}

fn last_error_message() -> String {
    // SAFETY: retrieves this thread's last Win32 error number.
    let code = unsafe { GetLastError() };
    format!("Win32 error {code}")
}

fn secret_store_last_error() -> SecretStoreError {
    // ERROR_NOT_FOUND is the documented Credential Manager result for a
    // missing generic credential. It intentionally becomes a typed state,
    // rather than a silent empty token.
    const ERROR_NOT_FOUND: u32 = 1_168;
    // SAFETY: retrieves this thread's last Win32 error number.
    let code = unsafe { GetLastError() };
    if code == ERROR_NOT_FOUND {
        SecretStoreError::NotFound
    } else {
        SecretStoreError::Unavailable(format!("Win32 error {code}"))
    }
}

fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
