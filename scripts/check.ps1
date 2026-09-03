param(
    [switch]$Release
)

$ErrorActionPreference = 'Stop'

./scripts/prepare-sidecars.ps1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features free-whisper-worker/test-fixture
pnpm ui:check
pnpm ui:test
pnpm ui:licenses
pnpm licenses:check

if ($Release) {
    cargo deny check --hide-inclusion-graph
    pnpm tauri:build
}
