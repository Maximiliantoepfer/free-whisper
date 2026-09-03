# Desktop command contract (vertical slice)

The Svelte UI calls only Tauri commands defined by the Rust desktop backend.
Commands either return the documented DTO or reject with
`{ "code": string, "message": string }`. Messages are user-facing and codes
are stable for UI handling; neither contains audio, transcript text, bearer
tokens or model-download URLs beyond the user-selected catalogue entry.

## Commands

| Command | Purpose |
| --- | --- |
| `runtime_snapshot` | Read manifest, active provider/model, microphone, persisted recording and Windows-safety preferences, runtime and latest transcript state. |
| `model_catalog` | Read only the verified signed CPU catalogue and merge local installation state. |
| `install_model`, `cancel_model_install`, `activate_model`, `delete_model` | Explicit model lifecycle. Activation rehashes an installed file without network access. |
| `start_recording`, `recording_status`, `stop_recording`, `cancel_current_job` | Capture and one job through the explicitly active local or personal remote provider. |
| `copy_transcript` | Explicitly copy immutable `final_text`; no auto-paste or clipboard restore occurs. |
| `paste_transcript_to_original` | Re-check an opt-in target snapshot, focus it only after a user confirmation, then paste or return a visible copy-only fallback. |
| `save_windows_integration_settings` | Persist non-secret Auto-Paste and clipboard-restore preferences plus the next-start hotkey binding. |
| `recent_transcripts` | Read the last 25 locally stored transcript records. |
| `search_transcripts`, `revert_transcript_correction` | Literal, parameterized history search and individual correction reverts derived from immutable raw text. |
| `lexicon_workspace`, `save_lexicon_profile`, `delete_lexicon_profile`, `set_active_lexicon_profile` | Read and maintain local profiles; only enabled profiles can become active. |
| `save_lexicon_entry`, `delete_lexicon_entry`, `import_lexicon`, `export_lexicon` | Conservative term/variant maintenance and fully validated CSV/JSON transfer. |
| `local_provider_status` | Read packaged local-worker, CPU-server and active-model readiness without starting a worker or sending audio. |
| `remote_provider_workspace` | Read non-sensitive metadata for configured personal workers; no token is returned. |
| `test_remote_provider` | Explicitly test URL policy, TLS/connection, bearer authentication, capabilities and models without persisting the token. |
| `save_remote_provider`, `activate_remote_provider`, `use_local_provider`, `delete_remote_provider` | Persist only tested remote-profile metadata, switch the explicit provider or remove a confirmed remote profile and its Windows credential. |

`start_recording` refuses a missing selected model, unavailable microphone,
remote credential/profile fault or a busy processor. A remote profile must have
been explicitly tested and activated. `stop_recording` refuses
empty/VAD-insufficient audio before it invokes the selected provider.

Recording preferences contain only microphone selection, mode, duration and
VAD parameters. They are saved as non-sensitive JSON in SQLite when recording
starts; malformed saved preferences are reported and replaced in memory with
safe defaults, never silently accepted.

## Events

Both event payloads have `apiVersion: 1` and must be rejected by the UI if a
future incompatible version is received.

- `desktop.v1.model-download-progress`: model ID, received/total bytes,
  throughput and optional remaining seconds.
- `desktop.v1.state`: job UUID, typed domain state and optional visible detail.
- `desktop.v1.platform-error`: Windows hotkey, target-window, tray, clipboard
  restore or shutdown error that requires visible UI feedback.

The event stream contains no PCM samples, transcript text or secrets.

`scripts/check-desktop-command-bindings.ps1` is a CI gate: it compares the
registered Rust commands with the typed TypeScript client and fails on missing
or unknown command names.
