# Privacy

## Default data flow

Microphone samples are captured in memory, normalised and sent to the selected
provider. The default provider is local. Audio files are not persisted. On a
successful transcription, SQLite stores only transcript text, selected model,
provider identity, durations, corrections and injection outcome.

The default desktop result flow writes text to the Windows clipboard only after
an explicit user click. Optional Auto-Paste is off by default. When the user
enables it, the app keeps an HWND/PID/process-start snapshot only for that
opt-in flow, refuses an elevated or changed target, and sends `Ctrl+V` only
after a second check. The window title is stored only when that opt-in flow
needs a later confirmed paste.

Before Auto-Paste, the app attempts a clipboard backup only when the complete
previous clipboard is exactly Unicode text. After 400 ms it restores that text
only if Windows' clipboard sequence counter did not change. Clipboard races,
non-text content and restore failures leave the result in the clipboard and
are recorded as a visible transcript warning; no non-text clipboard data is
ever guessed or overwritten.

```text
Microphone -> memory-only audio buffer -> local worker -> local model -> result
                                              |
                                              +-- only when configured: HTTPS personal worker
```

## Network policy

The application makes no telemetry, analytics, account, update or model
requests automatically. A network request is permitted only for a user-started
model download or a user-configured personal remote provider. Remote URLs must
use HTTPS except confirmed loopback development mode.

## Sensitive values

Remote bearer tokens are stored through the Windows Credential Manager
abstraction. SQLite provider profiles carry only its reference and typed
non-secret timeout/development settings; the settings repository rejects keys
or nested JSON fields named like tokens, secrets, passwords, credentials or
authorization. SQLite never contains remote tokens or audio; ordinary tracing
logs never contain tokens, audio, transcript content or diagnostic payloads. A
future diagnostic export will default to excluding sensitive content.

Before a remote provider is saved, the user must explicitly test its endpoint.
That test sends its bearer token only to the entered worker in order to call
health, capability and model endpoints; it is not stored unless the user then
clicks **Save & activate**. Each later recording visibly identifies the active
provider. Switching back to the local provider stops future audio transfers.
