# Threat model

free-whisper treats spoken audio, transcript text, remote bearer tokens and a
user's foreground-window context as sensitive assets.

| Asset / boundary | Primary threat | Mitigation |
| --- | --- | --- |
| Microphone audio | Unintended persistence or external transfer | Bounded in-memory recording only; no audio table; local provider by default; remote transfer needs a selected personal profile. |
| Remote token | Leakage through SQLite, logs or diagnostics | Windows Credential Manager generic credential; SQLite stores only an opaque `free-whisper/provider/<uuid>` reference; standard logs never include tokens. |
| Remote endpoint | Clear-text transfer or endpoint confusion | HTTPS-only validation; HTTP only for explicit localhost/loopback development mode; health, capabilities and models are tested before persistence. |
| Worker response | Protocol downgrade or malformed values | Every v1 response and error carries an enforced API version; unknown backend or capabilities fail visibly. |
| Clipboard / paste | Text injected into a changed or elevated target | Copy is default; optional paste verifies HWND, PID and process creation time, rejects elevated targets and uses a sequence-counter guarded restore. |
| Model catalogue | Download substitution | Embedded Ed25519 public key, scope-bound signed manifest, HTTPS and size/SHA-256 validation before atomic activation. |

Out of scope for the alpha: a compromised operating-system account, a
malicious remote worker that has already passed authentication, and enterprise
certificate interception. Operators of their own worker must protect its host,
TLS termination and bearer token; the app shows the selected remote provider so
this trust boundary is explicit.
