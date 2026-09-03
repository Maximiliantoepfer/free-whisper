param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$OutputDirectory = "apps/desktop/src-tauri/binaries"
)

$ErrorActionPreference = 'Stop'

if ($Target -ne 'x86_64-pc-windows-msvc') {
    throw "only the Windows x64 release sidecars are supported; got $Target"
}

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
& (Join-Path $root 'scripts/build-whisper-cpp.ps1') -OutputDirectory 'target/sidecars'
if ($LASTEXITCODE -ne 0) {
    throw 'whisper.cpp CPU server build failed'
}

Push-Location $root
try {
    & cargo build --locked --release --package free-whisper-worker --bin free-whisper-worker --target $Target
    if ($LASTEXITCODE -ne 0) {
        throw 'free-whisper-worker release build failed'
    }
} finally {
    Pop-Location
}

$output = [System.IO.Path]::GetFullPath((Join-Path $root $OutputDirectory))
$worker = Join-Path $root "target/$Target/release/free-whisper-worker.exe"
$server = Join-Path $root 'target/sidecars/whisper-server.exe'
foreach ($file in @($worker, $server)) {
    if (-not (Test-Path -LiteralPath $file)) {
        throw "expected sidecar was not produced: $file"
    }
}

New-Item -ItemType Directory -Force -Path $output | Out-Null
# Tauri resolves external binaries using this exact target-suffixed filename.
$stagedWorker = Join-Path $output "free-whisper-worker-$Target.exe"
Copy-Item -LiteralPath $worker -Destination $stagedWorker -Force
Copy-Item -LiteralPath $server -Destination (Join-Path $output 'whisper-server.exe') -Force
Get-ChildItem -LiteralPath (Join-Path $root 'target/sidecars') -File -Filter '*.dll' |
    ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $output $_.Name) -Force
    }

# A compiled sidecar is deliberately ignored by Git. Verify the actual staged
# executable so a developer build and an installer cannot silently reuse a
# worker compiled before the pinned verbose_json adapter changed.
& (Join-Path $root 'scripts/verify-sidecars.ps1') -WorkerPath $stagedWorker -RequireServer
