# free-whisper v2 implementation plan

## Objective

free-whisper v2 is an offline-first, MIT-licensed Windows x64 desktop
transcription application. The Rust backend owns all business logic; the
Tauri/Svelte frontend only renders typed state and invokes typed commands.
Legacy Python code is an import/reference source only and is kept under
`old/python/`.

## Delivery order

1. Establish the workspace, state machine, quality gates, and fakes.
2. Build SQLite persistence and conservative lexicon correction.
3. Define the versioned provider protocol and run whisper.cpp behind a managed
   worker process.
4. Add signed-manifest model installation, audio/VAD, then Windows integration.
5. Add the desktop UI, remote configuration, legacy import, benchmarks, and a
   reproducible Windows release workflow.

## Non-negotiable design rules

- A job carries its own UUID, immutable recording snapshot, and state machine;
  one job cannot change another job's state.
- SQLite is the only persistent application-data store. Credentials are stored
  by a platform secret-store abstraction.
- Audio is transient by default. Logs and diagnostics exclude transcript text,
  audio, bearer tokens, and credentials.
- The bounded queue permits one running and one waiting job; a full queue is a
  visible error, never a dropped recording.
- No model download occurs from a recording path. Missing or corrupt models
  produce a concrete installation action.
- The initial package contains CPU inference only. GPU never falls back to CPU
  silently.

## Planned workspace

```text
Cargo.toml                 Rust workspace root
crates/domain              types, invariants, state machines
crates/application         orchestration and bounded jobs
crates/storage             SQLite migrations and repositories
crates/lexicon             prompt selection and exact corrections
crates/providers           provider trait and clients
crates/protocol            versioned remote worker DTOs
crates/audio               CPAL capture, normalisation, VAD
crates/platform-windows    hotkey, foreground, clipboard, injection
crates/test-support        fakes and fixtures
apps/desktop               Tauri 2 backend and Svelte UI
apps/worker                managed/remote worker binary
resources/models           signed model manifest
```

## Milestone gate

Every milestone adds `docs/milestones/NN-*.md` with the changed files,
architecture decision, commands run, results, and residual risks. The next
milestone starts only after its report is committed to the worktree.
