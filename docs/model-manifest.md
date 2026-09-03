# Model manifest

`resources/models/manifest.json` is the shipped catalogue for explicit model
installation. It contains immutable HTTPS URLs, a source revision, byte size,
SHA-256, licence provenance, CPU backend and memory estimate for every model.
Models are never part of the installer and no manifest URL is contacted during
recording.

The signed `trust_scope` is part of the RFC-8785 canonical JSON. An Alpha
binary accepts only an `alpha` catalogue; a stable binary accepts only a
`stable` catalogue. This prevents a valid Alpha signing key from authorising a
stable release. CI signs with `FREE_WHISPER_MANIFEST_SIGNING_KEY` (base64
encoded 32-byte seed). The resulting detached base64 signature is saved as
`manifest.sig`; the matching public key is stored in `manifest.public-key` and
compiled into the desktop application. The manager rejects missing, malformed,
mismatched or wrong-channel signatures before it opens a download connection.

Downloads use HTTPS only. A `.partial` file and matching resume metadata are
kept only inside the user-selected model directory. The final model is
SHA-256-checked, moved into a unique staging directory and atomically activated.
Cancelled downloads remain resumable; invalid downloads never become active.

## Alpha catalogue

The checked-in Alpha catalogue is signed, but its private seed is not. On the
one developer machine that owns the Alpha identity, run:

```powershell
.\scripts\provision-alpha-manifest.ps1
```

The script creates `.env.alpha.local` with a cryptographically random seed if
needed, confirms that Git ignores it, signs the catalogue as `alpha`, and
removes the environment variable before it returns. It never prints the seed.
Only the public key and detached signature are committed. Do not copy the seed
into a shared `.env`, CI log, release artefact, SQLite database or diagnostics.

Verify the tracked Alpha artefacts without a private key:

```powershell
cargo run --locked -p free-whisper-model-manager --bin verify-model-manifest -- `
  resources/models/manifest.json resources/models/manifest.sig `
  resources/models/manifest.public-key --scope alpha
```

From the trusted stable-release environment only, run:

```powershell
cargo run --locked -p free-whisper-model-manager --bin sign-model-manifest -- `
  resources/models/manifest.json resources/models/manifest.sig `
  resources/models/manifest.public-key --scope stable
```

The command reads `FREE_WHISPER_MANIFEST_SIGNING_KEY` as a base64 32-byte seed,
writes only a detached signature and the non-secret public key, and never logs
the seed. Stable CI sets `FREE_WHISPER_MANIFEST_SCOPE=stable`; builds reject a
scope that conflicts with their package channel. Review the generated manifest,
signature and public key before building the installer. The private key belongs
exclusively in the release secret store.
