[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$secretPath = Join-Path $repositoryRoot '.env.alpha.local'
$manifestPath = Join-Path $repositoryRoot 'resources/models/manifest.json'
$signaturePath = Join-Path $repositoryRoot 'resources/models/manifest.sig'
$publicKeyPath = Join-Path $repositoryRoot 'resources/models/manifest.public-key'

# A developer machine keeps the private Alpha seed locally. The public key and
# detached signature are deliberately not secret and are compiled into Alpha
# builds. This script never prints the seed or exports it beyond this process.
& git -C $repositoryRoot check-ignore --quiet -- .env.alpha.local
if ($LASTEXITCODE -ne 0) {
    throw '.env.alpha.local is not ignored; refusing to create a signing secret'
}

if (Test-Path -LiteralPath $secretPath) {
    $line = Get-Content -LiteralPath $secretPath | Where-Object {
        $_ -match '^FREE_WHISPER_MANIFEST_SIGNING_KEY=(.+)$'
    } | Select-Object -First 1
    if ($null -eq $line) {
        throw '.env.alpha.local does not contain FREE_WHISPER_MANIFEST_SIGNING_KEY'
    }
    $seed = ($line -replace '^FREE_WHISPER_MANIFEST_SIGNING_KEY=', '').Trim()
} else {
    $bytes = [byte[]]::new(32)
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
    $seed = [Convert]::ToBase64String($bytes)
    $content = "# Local Alpha signing secret. Never commit this file.`nFREE_WHISPER_MANIFEST_SIGNING_KEY=$seed`n"
    [System.IO.File]::WriteAllText(
        $secretPath,
        $content,
        [System.Text.UTF8Encoding]::new($false)
    )
}

try {
    $env:FREE_WHISPER_MANIFEST_SIGNING_KEY = $seed
    & cargo run --locked -p free-whisper-model-manager --bin sign-model-manifest -- `
        $manifestPath $signaturePath $publicKeyPath --scope alpha
    if ($LASTEXITCODE -ne 0) {
        throw 'Alpha model manifest signing failed'
    }
} finally {
    Remove-Item Env:FREE_WHISPER_MANIFEST_SIGNING_KEY -ErrorAction SilentlyContinue
    Remove-Variable seed -ErrorAction SilentlyContinue
    Remove-Variable bytes -ErrorAction SilentlyContinue
}

Write-Output 'Alpha manifest provisioned. The private seed remains only in .env.alpha.local.'
