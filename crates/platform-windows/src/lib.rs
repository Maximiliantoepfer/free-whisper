#![deny(unsafe_op_in_unsafe_fn)]

//! Small, auditable Windows-only integration boundary.
//!
//! All Win32 calls live in `native.rs`; policy and value types remain safe Rust
//! so they can be tested without a desktop session. The rest of the workspace
//! must never call Win32 directly.

use free_whisper_domain::TargetWindowSnapshot;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(target_os = "windows")]
mod native;

#[cfg(target_os = "windows")]
pub use native::{
    GlobalHotkey, PasteRefusalOrError, WindowsClipboard, WindowsCredentialStore, WindowsDesktop,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyKey {
    Space,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBinding {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: HotkeyKey,
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self {
            control: true,
            alt: false,
            shift: false,
            key: HotkeyKey::Space,
        }
    }
}

impl HotkeyBinding {
    #[must_use]
    pub fn display_name(&self) -> String {
        let mut segments = Vec::with_capacity(4);
        if self.control {
            segments.push("Ctrl");
        }
        if self.alt {
            segments.push("Alt");
        }
        if self.shift {
            segments.push("Shift");
        }
        segments.push(match self.key {
            HotkeyKey::Space => "Leertaste",
            HotkeyKey::F8 => "F8",
            HotkeyKey::F9 => "F9",
            HotkeyKey::F10 => "F10",
            HotkeyKey::F11 => "F11",
            HotkeyKey::F12 => "F12",
        });
        segments.join("+")
    }

    pub fn validate(&self) -> Result<(), HotkeyError> {
        if !self.control && !self.alt && !self.shift {
            return Err(HotkeyError::InvalidBinding(
                "at least one modifier key is required".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub const fn virtual_key(self) -> u32 {
        match self.key {
            HotkeyKey::Space => 0x20,
            HotkeyKey::F8 => 0x77,
            HotkeyKey::F9 => 0x78,
            HotkeyKey::F10 => 0x79,
            HotkeyKey::F11 => 0x7A,
            HotkeyKey::F12 => 0x7B,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HotkeyEvent {
    Pressed,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum HotkeyError {
    #[error("invalid global hotkey: {0}")]
    InvalidBinding(String),
    #[error(
        "the global hotkey could not be registered; another application may already use it ({0})"
    )]
    RegistrationFailed(String),
    #[error("the global hotkey message loop did not start: {0}")]
    StartupFailed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForegroundWindow {
    pub snapshot: TargetWindowSnapshot,
    pub elevated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PasteRefusal {
    NoTargetWindow,
    WindowChanged,
    ElevatedTarget,
    FocusDenied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PasteDecision {
    Allowed,
    Refused(PasteRefusal),
}

/// Stateless safety policy for automatic and confirmed paste. A PID alone is
/// insufficient because Windows can reuse it, so creation FILETIME is checked.
#[must_use]
pub fn evaluate_paste_policy(
    expected: Option<&TargetWindowSnapshot>,
    foreground: Option<&ForegroundWindow>,
) -> PasteDecision {
    let Some(expected) = expected else {
        return PasteDecision::Refused(PasteRefusal::NoTargetWindow);
    };
    let Some(foreground) = foreground else {
        return PasteDecision::Refused(PasteRefusal::WindowChanged);
    };
    if foreground.elevated {
        return PasteDecision::Refused(PasteRefusal::ElevatedTarget);
    }
    if foreground.snapshot.window_handle != expected.window_handle
        || foreground.snapshot.process_id != expected.process_id
        || foreground.snapshot.process_started_at_filetime != expected.process_started_at_filetime
    {
        return PasteDecision::Refused(PasteRefusal::WindowChanged);
    }
    PasteDecision::Allowed
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ClipboardError {
    #[error("text to copy must not be empty")]
    EmptyText,
    #[error("the Windows clipboard is unavailable: {0}")]
    Unavailable(String),
}

pub trait ClipboardWriter: Send + Sync {
    fn copy_text(&self, text: &str) -> Result<(), ClipboardError>;
}

/// Stores bearer tokens outside SQLite. References are deliberately opaque and
/// carry no endpoint or secret material. The desktop layer may persist only the
/// reference, never the token returned by [`SecretStore::read_secret`].
pub trait SecretStore: Send + Sync {
    fn write_secret(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError>;
    fn read_secret(&self, reference: &str) -> Result<String, SecretStoreError>;
    fn delete_secret(&self, reference: &str) -> Result<(), SecretStoreError>;
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SecretStoreError {
    #[error("invalid credential reference")]
    InvalidReference,
    #[error("invalid credential value")]
    InvalidSecret,
    #[error("credential was not found")]
    NotFound,
    #[error("Windows Credential Manager is unavailable: {0}")]
    Unavailable(String),
}

/// Credential references are fixed to our namespace so this application never
/// reads or mutates unrelated Windows credentials.
pub fn validate_credential_reference(reference: &str) -> Result<(), SecretStoreError> {
    if !reference.starts_with("free-whisper/provider/")
        || reference.len() > 256
        || reference.len() <= "free-whisper/provider/".len()
        || reference.chars().any(char::is_control)
    {
        return Err(SecretStoreError::InvalidReference);
    }
    Ok(())
}

pub fn validate_credential_secret(secret: &str) -> Result<(), SecretStoreError> {
    if !(16..=512).contains(&secret.len()) || secret.chars().any(char::is_control) {
        return Err(SecretStoreError::InvalidSecret);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardRestoreTicket {
    pub expected_sequence: u32,
    pub original_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipboardRestoreOutcome {
    Restored,
    NotRestorable,
    SkippedSequenceChanged,
    Failed(String),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum WindowError {
    #[error("Windows foreground-window inspection failed: {0}")]
    Inspection(String),
    #[error("Windows input injection failed: {0}")]
    Injection(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> TargetWindowSnapshot {
        TargetWindowSnapshot {
            window_handle: 41,
            process_id: 73,
            process_started_at_filetime: 99,
            title: "Notepad".to_owned(),
        }
    }

    #[test]
    fn default_hotkey_is_the_documented_toggle_binding() {
        assert_eq!(
            HotkeyBinding::default().display_name(),
            "Ctrl+Leertaste"
        );
        assert!(HotkeyBinding::default().validate().is_ok());
    }

    #[test]
    fn hotkey_virtual_keys_are_explicit_and_auditable() {
        assert_eq!(
            HotkeyBinding {
                control: true,
                alt: false,
                shift: false,
                key: HotkeyKey::F12,
            }
            .virtual_key(),
            0x7B
        );
    }

    #[test]
    fn hotkey_rejects_a_global_key_without_modifiers() {
        let binding = HotkeyBinding {
            control: false,
            alt: false,
            shift: false,
            key: HotkeyKey::Space,
        };
        assert!(matches!(
            binding.validate(),
            Err(HotkeyError::InvalidBinding(_))
        ));
    }

    #[test]
    fn paste_requires_exact_window_process_and_creation_time() {
        let expected = snapshot();
        let exact = ForegroundWindow {
            snapshot: expected.clone(),
            elevated: false,
        };
        assert_eq!(
            evaluate_paste_policy(Some(&expected), Some(&exact)),
            PasteDecision::Allowed
        );
        let changed_pid = ForegroundWindow {
            snapshot: TargetWindowSnapshot {
                process_id: 74,
                ..expected.clone()
            },
            elevated: false,
        };
        assert_eq!(
            evaluate_paste_policy(Some(&expected), Some(&changed_pid)),
            PasteDecision::Refused(PasteRefusal::WindowChanged)
        );
    }

    #[test]
    fn paste_refuses_elevated_targets() {
        let expected = snapshot();
        let elevated = ForegroundWindow {
            snapshot: expected.clone(),
            elevated: true,
        };
        assert_eq!(
            evaluate_paste_policy(Some(&expected), Some(&elevated)),
            PasteDecision::Refused(PasteRefusal::ElevatedTarget)
        );
    }

    #[test]
    fn paste_refuses_missing_or_changed_foreground_context() {
        let expected = snapshot();
        assert_eq!(
            evaluate_paste_policy(None, None),
            PasteDecision::Refused(PasteRefusal::NoTargetWindow)
        );
        assert_eq!(
            evaluate_paste_policy(Some(&expected), None),
            PasteDecision::Refused(PasteRefusal::WindowChanged)
        );
    }

    #[test]
    fn credential_namespace_and_token_bounds_are_explicit() {
        assert!(validate_credential_reference("free-whisper/provider/abc").is_ok());
        assert!(matches!(
            validate_credential_reference("other-app/provider/abc"),
            Err(SecretStoreError::InvalidReference)
        ));
        assert!(validate_credential_secret("x".repeat(16).as_str()).is_ok());
        assert!(matches!(
            validate_credential_secret("short"),
            Err(SecretStoreError::InvalidSecret)
        ));
    }
}
