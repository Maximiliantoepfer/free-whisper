# License inventory and distribution policy

## Product

The project remains MIT licensed. `LICENSES/THIRD_PARTY_NOTICES.md` is a
deterministic attribution inventory generated from the locked Rust and
JavaScript dependency graphs and is checked for drift in CI.

## Approved initial runtime classes

| Component | Planned use | License review status |
| --- | --- | --- |
| Tauri 2 | desktop shell | MIT/Apache-2.0; verify locked release |
| Svelte | UI | MIT; verify locked release |
| CPAL | audio input | Apache-2.0; verify locked release |
| Rubato | deterministic PCM resampling | MIT OR Apache-2.0; verify locked release |
| wavekat-vad (WebRTC feature only) | local WebRTC VAD wrapper | Apache-2.0 wrapper; audit its locked WebRTC dependency and notice before release |
| arboard | explicit Windows clipboard write | MIT OR Apache-2.0; verify locked release |
| clipboard-win / error-code | Windows clipboard implementation used by arboard | BSL-1.0; permissive licence admitted after review and included in generated notices |
| SQLite/rusqlite | local data | verify locked release and bundled SQLite options |
| whisper.cpp `v1.8.6` (`23ee035`) | separately managed CPU engine | MIT; source is a pinned submodule; build only `whisper-server` without FFmpeg |
| Whisper model weights | separately downloaded models | MIT source attribution and per-model manifest review |

## Exclusions

PyQt6, Python, faster-whisper, CTranslate2, ONNX Runtime, PyInstaller and
FFmpeg are not part of the v2 desktop release. The worker accepts already
normalised WAV/PCM; it does not bundle FFmpeg conversion.

The VAD dependency is pinned with only its `webrtc` feature. Its optional
Silero, TEN-VAD and FireRedVAD features are prohibited because they fetch ONNX
model files during builds and do not belong in the CPU desktop release.

## Enforcement

`cargo deny` checks Rust licenses, bans, sources and advisories. The JavaScript
lockfile is checked with a license report. A release fails if a distributable
dependency lacks an approved licence or notice.

The initial Tauri dependency graph includes file-level MPL-2.0 components
(`cssparser`, `selectors` and support crates). MPL-2.0 is explicitly approved
for unmodified upstream dependency files; their notices remain in the release
notice output. The policy checks only `x86_64-pc-windows-msvc`: GTK3 and related
Linux-only dependencies must not gate a Windows-only release and are not part
of its artifact. Adding another target requires extending the policy and
reviewing its complete graph first.

The Windows graph also includes Unicode-3.0 and Zlib licensed utility crates,
which are explicitly approved and must appear in generated notices. Five
RUSTSEC entries for unmaintained `unic-*` crates are accepted only because they
are an unavoidable Tauri `urlpattern` transitive dependency with no safe
upstream replacement. Each ID, reason, and review-on-Tauri-update requirement
is recorded in `deny.toml`; this is not a suppression of a known vulnerability.

The v1 worker/client uses reqwest with Rustls. Its resolved runtime graph adds
ISC-licensed `ring`, `rustls-webpki` and `untrusted`, plus
CDLA-Permissive-2.0-licensed `webpki-roots` public root-certificate data. Both
licenses are explicitly admitted in `deny.toml`, reviewed in CI and must appear
in the generated release notice.

The explicit clipboard adapter resolves to `clipboard-win` and `error-code`,
both under BSL-1.0. That licence permits use, distribution and derivative work
while preserving its notice; it is therefore admitted in `deny.toml` and kept
in the deterministic notice inventory. The application never uses this path
for automatic paste or clipboard restoration in the current slice.
