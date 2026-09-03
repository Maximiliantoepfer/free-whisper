# free-whisper v2

Free, open-source, offline-first transcription for Windows. V2 is a Rust/Tauri
desktop application that records audio locally, transcribes through a managed
local worker, applies conservative lexicon corrections and copies the result
only after an explicit user action.

## Status

V2 is under active reimplementation. The prior Python/PyQt release is archived
at [`old/python/`](old/python/) for data migration and historical reference; it
is not part of the v2 build or runtime.

## Principles

- No account, telemetry, or compulsory cloud service.
- Models are downloaded only after confirmation and operate offline afterwards.
- Audio leaves the device only for an explicitly configured remote provider.
- Transcript corrections are exact, visible, and individually reversible.
- Secrets use the Windows Credential Manager rather than SQLite or logs.

See [architecture](docs/architecture.md), [privacy](docs/privacy.md), and the
[implementation plan](docs/planning/v2-implementation.md).

The shipped Alpha catalogue is Ed25519-signed and can therefore install models
after an explicit click. Contributors who change the catalogue provision their
own local Alpha signing seed with
`./scripts/provision-alpha-manifest.ps1`; the generated `.env.alpha.local` is
ignored and never enters the product. Stable releases use a separate secret and
reject Alpha catalogues. See [the model-manifest guide](docs/model-manifest.md).

## Current vertical slice

After the signed Alpha manifest has been provisioned, the desktop app supports:

- choosing Tiny, Base or Small with download size, RAM estimate, license and
  source information;
- resumable, checksummed installation and explicit activation of one local CPU
  model;
- microphone selection, bounded local capture, 16-kHz mono normalisation and
  voice/silence validation;
- local whisper.cpp worker transcription, conservative lexicon corrections,
  local transcript history and explicit clipboard copying;
- Windows tray lifecycle and the global default hotkey `Ctrl+Alt+Leertaste`;
- optional Auto-Paste, guarded by an exact HWND/PID/process-start snapshot,
  UAC detection and a conditional Windows clipboard sequence-counter restore.
- a local lexicon workspace with profiles, explicit variants, transactional
  CSV/JSON import/export, literal transcript search and individual correction
  reverts;
- local provider readiness without starting a worker or transferring audio.
- a tested, explicitly selected personal remote worker with HTTPS-by-default,
  selected ready model and a bearer token held only in Windows Credential
  Manager.

The confirmed legacy migration assistant, benchmark corpus and release report
remain M9 work. The release-security gate for the signed model catalogue remains
unchanged.

## Development bootstrap

Windows x64 requires Rust 1.97.1 with the MSVC target, Visual Studio C++ Build
Tools, CMake, Node.js 22.12.0, and pnpm 11.19.0. Once dependencies are installed:

```powershell
pnpm install --frozen-lockfile
./scripts/prepare-sidecars.ps1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features free-whisper-worker/test-fixture
pnpm ui:check
pnpm ui:test
pnpm licenses:check
```

`pnpm tauri:build` creates the Windows x64 NSIS installer and always rebuilds
and verifies the CPU sidecars first. Use Node.js **22.12.0** exactly;
`.node-version` and CI enforce that version.

For an interactive development window, run:

```powershell
pnpm tauri:dev
```

If `pnpm` is not globally available, Corepack can run the pinned package
manager without an administrator-installed shim:

```powershell
corepack pnpm install --frozen-lockfile
corepack pnpm tauri:dev
```

`tauri:dev` and `tauri:build` run sidecar preparation automatically. A direct
Tauri invocation performs only a preflight and fails with the root command to
run if the generated worker is missing or stale. The worker reports a token-free
build identity, so an old executable cannot silently use a previous engine
adapter.

Run `./scripts/provision-alpha-manifest.ps1` only when changing the Alpha
catalogue. A real local transcription needs an explicitly installed model and
a working microphone.


## Developer Start

```bash
pnpm install --frozen-lockfile
pnpm tauri:dev
```

```bash
corepack pnpm --version
corepack pnpm install --frozen-lockfile
corepack pnpm tauri:dev
```
