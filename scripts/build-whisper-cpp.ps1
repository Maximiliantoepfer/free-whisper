param(
    [string]$SourceDirectory = "third_party/whisper.cpp",
    [string]$BuildDirectory = "target/whisper-cpp",
    [string]$OutputDirectory = "target/sidecars"
)

$ErrorActionPreference = 'Stop'

$source = (Resolve-Path -LiteralPath $SourceDirectory).Path
$build = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $BuildDirectory))
$output = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputDirectory))
$cmake = (Get-Command cmake -ErrorAction SilentlyContinue).Source
if ($null -eq $cmake) {
    $cmake = 'C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
}
if (-not (Test-Path -LiteralPath $cmake)) {
    throw 'CMake is required to build the whisper.cpp CPU sidecar'
}

if (-not (Test-Path -LiteralPath (Join-Path $source '.git'))) {
    throw "whisper.cpp submodule is missing: $source"
}

# The submodule can be owned by a different local Windows account than the
# terminal process (for example a sandboxed CI runner). Scope the trust
# exception to this read-only pin check instead of modifying global Git config.
$gitSafeSource = $source.Replace('\', '/')
$commit = (& git -c "safe.directory=$gitSafeSource" -C $source rev-parse HEAD).Trim()
if ($commit -ne '23ee03506a91ac3d3f0071b40e66a430eebdfa1d') {
    throw "whisper.cpp must be pinned to 23ee035; found $commit"
}

# whisper-server is declared by examples/server in v1.8.6. Building the target
# below produces only that executable; FFmpeg support remains disabled. CMake
# asks the vendored source for its Git revision, so pass the same one-process
# safe-directory exception without changing a developer's global Git config.
& $cmake -E env "GIT_CONFIG_COUNT=1" "GIT_CONFIG_KEY_0=safe.directory" "GIT_CONFIG_VALUE_0=$gitSafeSource" $cmake -S $source -B $build -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON -DWHISPER_BUILD_SERVER=ON -DWHISPER_COMMON_FFMPEG=OFF
& $cmake --build $build --config Release --target whisper-server

$server = Get-ChildItem -LiteralPath $build -Recurse -File -Filter 'whisper-server.exe' |
    Select-Object -First 1
if ($null -eq $server) {
    throw 'whisper.cpp build completed without whisper-server.exe'
}

New-Item -ItemType Directory -Force -Path $output | Out-Null
Copy-Item -LiteralPath $server.FullName -Destination (Join-Path $output 'whisper-server.exe') -Force

# v1.8.6 builds the CPU backend as sibling DLLs on Windows. They must travel
# with whisper-server; copying only the executable would fail at process start.
$runtimeLibraries = Get-ChildItem -LiteralPath $build -Recurse -File -Filter '*.dll' |
    Where-Object { $_.DirectoryName -match '\\bin\\Release$' }
if ($runtimeLibraries.Count -eq 0) {
    throw 'whisper.cpp server build completed without its runtime DLLs'
}
foreach ($library in $runtimeLibraries) {
    Copy-Item -LiteralPath $library.FullName -Destination (Join-Path $output $library.Name) -Force
}
