# Contributing to free-whisper v2

## Setup

Use the pinned Rust toolchain from `rust-toolchain.toml`, Windows MSVC C++ Build
Tools, CMake, Node 20+, and the pnpm version declared in `package.json`.

## Required checks

Run `scripts/check.ps1` before proposing a change. Do not suppress Clippy
warnings, log user audio/text/tokens, or add a network request without a
documented opt-in path.

## Design boundaries

- Keep domain invariants in `crates/domain`.
- Keep UI code free of business decisions.
- Return typed errors to the UI; do not use catch-all recovery.
- Add migration, provider, or lexicon behaviour only with tests.
- Do not add Python/PyQt dependencies to v2.
