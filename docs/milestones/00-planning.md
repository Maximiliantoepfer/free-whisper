# Milestone 0 — Planning and risk inventory

**Status:** complete (2026-08-21)

## Changed files

- `docs/planning/v2-implementation.md`
- `docs/legacy-inventory.md`
- `docs/license-inventory.md`
- `docs/platform-risks.md`
- `docs/adr/0001-rust-tauri.md` through `docs/adr/0006-legacy-migration.md`

## Decisions

V2 is a Rust/Tauri product with SQLite as its sole persistent data store, an
opt-in legacy importer, exact lexicon corrections, and a separately managed
whisper.cpp worker. The existing Python application remains unchanged in this
milestone.

## Checks run

- `git -c safe.directory=C:/Users/maxto/source/free-whisper diff --check` — passed.
- Required planning/ADR file existence check — passed.
- Markdown heading scan with `rg '^#' docs` — passed.

## Open risks

The local machine currently exposes Rust 1.71 and no detected MSVC compiler or
CMake. M1 cannot pass its build gate until the pinned toolchain and native
Windows build prerequisites are available.
