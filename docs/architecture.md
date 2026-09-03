# Architecture

## Runtime boundaries

The desktop process contains Tauri, local SQLite, bounded audio capture,
provider clients and backend-only job orchestration. It owns the sole
authoritative `TranscriptionJob` state machine; Svelte receives typed
snapshots/events and never owns audio, worker or persistence state.

The managed worker is a separate process. For local use it binds only to a
random loopback port and authenticates with an ephemeral token. For remote use
it exposes the same versioned protocol through HTTPS. whisper.cpp remains an
inner process of that worker; the desktop application never links it via FFI.

```text
Svelte UI --typed commands/events--> Tauri backend --> job controller
                                                   |-> SQLite repositories
                                                   |-> Windows integration
                                                   |-> signed ModelManager --> model directory
                                                   |-> provider trait --> worker --> whisper.cpp
```

## State ownership

`crates/domain` defines the legal state graph. `JobStateRegistry` maps a job
UUID to exactly one state machine and rejects an event attributed to another
job. The current interactive slice accepts one active capture or inference job
and returns a visible `queue_full` error rather than retaining unbounded audio.
The application crate's capacity-one queue is mirrored by the desktop
controller: it permits one active inference plus one finalized, bounded audio
recording waiting for that inference. A third finalized recording yields the
visible `queue_full` error. Waiters use a notification after provider cleanup;
no asynchronous operation occurs while a state mutex is held.

## Persistence and lexicon

`crates/storage` opens SQLite with foreign keys, trusted-schema protection and
versioned, immediate transactions. It is the only persistent store for settings,
provider profiles, installed models, lexicon data, transcripts, corrections and
migration audit records. A provider profile can carry only a Credential Manager
reference plus typed non-secret connection settings; token-like settings keys are
rejected before a write.

`crates/lexicon` normalises user terms to NFC, deduplicates canonical entries by
case-folded identity and limits model hints deterministically to 32 terms and an
estimated 128-token ceiling. Post-correction has no fuzzy or regex mode: it
replaces explicit variants only at Unicode word boundaries. Each replacement is
recorded against immutable raw text, so reverting one correction rebuilds the
final text rather than editing an already changed string.

## Model installation

`crates/model-manager` accepts a catalogue only after RFC-8785 canonical JSON
and its detached Ed25519 signature verify against the release public key. A
Tauri command is the only download entry point; it emits typed progress with
throughput and ETA, supports cancellation, and persists only verified model
metadata to SQLite. Model bytes live in a configured model directory outside
the installer and are activated through a unique staging directory only after
size and SHA-256 validation. The current CPU-only manifest uses typed backend
values; unsupported GPU entries are rejected rather than falling back.

The active lexicon profile is a non-sensitive SQLite setting. Global entries
and enabled entries belonging to that profile are snapshotted at job finalising;
the same snapshot controls both prompt hints and conservative exact-variant
post-correction. The UI cannot introduce fuzzy or regular-expression rules.

The desktop starts a packaged `free-whisper-worker` sidecar on `127.0.0.1:0`.
The worker prints exactly one token-free ready line containing its actual
loopback address, so the desktop no longer reserves and races a port. Its
inner whisper.cpp adapter requests `verbose_json`, maps returned segments and
language when available, and reports measured inference time. It does not
advertise word timestamps or language detection as guaranteed capabilities
where the unversioned engine cannot prove them.

## Vertical-slice recording flow

1. `start_recording` validates an active model, launches the local sidecar and
   checks readiness before CPAL opens the selected microphone.
2. CPAL retains only bounded in-memory data. Its live WebRTC VAD marks toggle
   capture after configured silence; on stop, Rubato normalises to 16-kHz mono
   PCM-f32 and the same WebRTC VAD preflight rejects silence before a provider
   request. See [ADR 0007](adr/0007-audio-vad.md).
3. The worker transcribes locally. Active lexicon entries create a bounded
   prompt snapshot; exact variants then become visible corrections.
4. SQLite stores raw/final text, segments, provider data, durations and a
   visible injection outcome. An explicit clipboard command writes final text
   and records `CopiedToClipboard` or `FailedWithReason`.

Audio bytes are released after the provider request and are never written to
SQLite or transcript history.

## Windows boundary and safe paste

`crates/platform-windows` is the sole Win32 FFI boundary. Its pure policy
compares HWND, PID and process creation FILETIME before any paste and refuses
elevated targets. The global `Ctrl+Alt+Leertaste` binding has its own Windows
message loop with `MOD_NOREPEAT`; registration failures are runtime errors,
not silent fallbacks. Tray actions, the hotkey and the window controls all use
the same Rust recording controller functions.

Copy is the default. When Auto-Paste is explicitly enabled, a transient target
snapshot is captured at recording start. The text is copied, the original
context is checked again, then `Ctrl+V` is injected only if it still matches.
An original clipboard value is kept only if it contained exactly Unicode text;
after 400 ms it is restored only when the Windows clipboard sequence counter is
unchanged. Any skipped restoration or paste refusal is persisted as a visible
transcript warning. Target-window data is stored only for this opt-in flow.

## Product assets

`resources/brand/free-whisper-signal-master.png` is the dedicated v2 master
asset. Tauri-generated ICO and PNG variants under `apps/desktop/src-tauri/icons/`
are used by the Windows executable and tray; no asset from the Python legacy is
used by the v2 product.
