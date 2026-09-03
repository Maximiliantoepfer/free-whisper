[CmdletBinding()]
param(
    [string]$WorkerPath,
    [switch]$RequireServer
)

$ErrorActionPreference = 'Stop'

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$target = 'x86_64-pc-windows-msvc'
if (-not $WorkerPath) {
    $WorkerPath = Join-Path $root "apps/desktop/src-tauri/binaries/free-whisper-worker-$target.exe"
}

$cargoManifest = Get-Content -Raw -LiteralPath (Join-Path $root 'Cargo.toml')
$versionMatch = [regex]::Match($cargoManifest, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw 'cannot determine the workspace package version for the sidecar check'
}
$expected = "free-whisper-worker|$($versionMatch.Groups[1].Value)|1|whispercpp-v1.8.6-verbose-json-start-end"

if (-not (Test-Path -LiteralPath $WorkerPath -PathType Leaf)) {
    throw "Local sidecar is missing: $WorkerPath. Run 'corepack pnpm tauri:dev' from the repository root; it prepares sidecars first."
}
if ($RequireServer) {
    $server = Join-Path (Split-Path -Parent $WorkerPath) 'whisper-server.exe'
    if (-not (Test-Path -LiteralPath $server -PathType Leaf)) {
        throw "whisper.cpp sidecar is missing: $server. Run 'corepack pnpm tauri:dev' from the repository root."
    }
}

$actualLines = @(& $WorkerPath --build-info)
if ($LASTEXITCODE -ne 0) {
    throw "Local worker did not provide build information. Run 'corepack pnpm tauri:dev' from the repository root to rebuild it."
}
$actual = [string]::Join("`n", $actualLines).Trim()
if ($actual -ne $expected) {
    throw "Local worker is stale or incompatible (expected '$expected', received '$actual'). Run 'corepack pnpm tauri:dev' from the repository root or install the current installer."
}

Write-Output "Sidecar preflight passed: $actual"
