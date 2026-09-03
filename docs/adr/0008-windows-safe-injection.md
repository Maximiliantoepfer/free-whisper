# ADR 0008: narrow Win32 boundary and confirm-first paste

**Status:** Accepted (M6)

## Context

Global hotkeys, window targeting, clipboard restoration and input injection
cross process and integrity boundaries on Windows. Treating a current
foreground process id or a clipboard write as sufficient would permit a result
to land in the wrong application or overwrite a user's newer clipboard value.

## Decision

The `platform-windows` crate is the only direct Win32 boundary. Its native
module owns `RegisterHotKey`, foreground-window inspection, process creation
time and elevation queries, clipboard sequence access and `SendInput`. The
rest of the crate exposes safe value types and a pure policy that requires an
exact HWND, PID and process-start FILETIME match and rejects elevated targets.

`Ctrl+Alt+Leertaste` is the default toggle hotkey. It uses an isolated message
loop, `MOD_NOREPEAT` and guaranteed `UnregisterHotKey` on shutdown. A failed
registration is shown to the user; there is no alternate hidden binding.

Copy is the default. Auto-Paste is opt-in. It writes result text to the
clipboard, checks the target a second time and injects `Ctrl+V` only into the
unchanged, non-elevated target. A confirmation action may request focus of the
saved original window, but verifies the same identity again before injection.

The prior clipboard is backed up only if it consists entirely of Unicode text.
After 400 ms it is restored only if Windows reports the same clipboard sequence
counter. Other formats, a counter change and restore failures leave the result
in the clipboard and become visible transcript warnings.

## Consequences

- The default path cannot auto-paste into a different active window.
- Non-text clipboard values are intentionally not restored, avoiding lossy
  guesses about files, images, HTML or application-private formats.
- Window titles are persisted only for opt-in original-window paste records.
- Runtime tests cover pure policy, while a Windows manual gate remains needed
  for actual hotkey registration, Notepad paste and Clipboard/UAC behaviour.
