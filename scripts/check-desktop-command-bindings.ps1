[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$backend = Get-Content -Raw -LiteralPath (Join-Path $repositoryRoot 'apps/desktop/src-tauri/src/lib.rs')
$frontend = Get-Content -Raw -LiteralPath (Join-Path $repositoryRoot 'apps/desktop/ui/src/lib/api/desktop.ts')

$handler = [regex]::Match($backend, 'generate_handler!\[([\s\S]*?)\]\)')
if (-not $handler.Success) { throw 'Tauri command handler was not found' }
$backendCommands = [regex]::Matches($handler.Groups[1].Value, '\b[a-z][a-z0-9_]+\b') |
    ForEach-Object Value | Sort-Object -Unique
$frontendCommands = [regex]::Matches($frontend, "invoke(?:<[^>]+>)?\('([a-z][a-z0-9_]*)'") |
    ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique

$missingFromFrontend = Compare-Object $backendCommands $frontendCommands -PassThru |
    Where-Object { $_ -in $backendCommands }
$unknownToBackend = Compare-Object $frontendCommands $backendCommands -PassThru |
    Where-Object { $_ -in $frontendCommands }
if ($missingFromFrontend -or $unknownToBackend) {
    if ($missingFromFrontend) { Write-Error "Missing TypeScript command bindings: $($missingFromFrontend -join ', ')" }
    if ($unknownToBackend) { Write-Error "Unknown TypeScript command bindings: $($unknownToBackend -join ', ')" }
    throw 'Desktop command bindings have drifted'
}

$requiredBackendFields = @('sequence: u64', 'capture_phase: CapturePhaseView', 'active_job_id: Option<String>')
$requiredFrontendFields = @('sequence: number', 'capturePhase: CapturePhase', 'activeJobId: string | null')
foreach ($field in $requiredBackendFields) {
    if (-not $backend.Contains($field)) { throw "Desktop backend DTO field is missing: $field" }
}
foreach ($field in $requiredFrontendFields) {
    if (-not $frontend.Contains($field)) { throw "Desktop TypeScript DTO field is missing: $field" }
}

Write-Output "Desktop command binding check passed ($($backendCommands.Count) commands)."
