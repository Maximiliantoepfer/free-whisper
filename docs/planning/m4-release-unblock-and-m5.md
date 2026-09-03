# M4 release unblock and M5 execution plan

## M4 release unblock

1. In the protected release environment, provision
   `FREE_WHISPER_MANIFEST_SIGNING_KEY` as a new Ed25519 base64 seed. It must
   never enter the repository, a developer shell history, SQLite, diagnostics
   or CI logs.
2. Run `sign-model-manifest` to replace
   `resources/models/manifest.sig` and `manifest.public-key`; review that the
   JSON did not change unexpectedly and validate the catalogue with the desktop
   `model_catalog` command.
3. Run the M4 test matrix on Node 22.12.0: valid and manipulated signatures,
   HTTPS download, interrupted Range/ETag resume, cancellation during connect
   and stream, size/hash failures, destination race, metadata persistence and
   confirmation-gated deletion. Use a local TLS fixture with a test root, not
   a production model download.
4. Re-run `./scripts/check.ps1 -Release` on a clean Windows x64 runner and
   inspect the NSIS archive for `free-whisper-worker.exe`, `whisper-server.exe`
   and the required CPU DLLs. Publish no installer while the manifest sentinel
   remains present.

## M5 audio and orchestration

After the signed M4 gate is green, implement M5 in this order:

1. Implement device enumeration, CPAL callback error propagation, bounded
   15–600 second ring buffer and sample-format conversion in `crates/audio`.
2. Add deterministic 16-kHz mono resampling fixtures and choose a maintained,
   Windows-buildable VAD after its license review. Keep one shared VAD settings
   type for UI, live capture and pre-provider validation.
3. Extend the application coordinator with recording/finalising/model-loading
   transitions, bounded queue behavior, cancellation and duration/speech-share
   validation. Empty audio must yield a visible no-inference result.
4. Persist only transcript metadata, timings and visible errors. Add fake
   device loss, overflow, VAD silence, queue and cancellation integration tests.
5. Do not start M6 Windows injection work until M5 passes formatting, Clippy,
   workspace tests, Node 22.12 frontend gates, license drift checks and a clean
   Windows NSIS build.
