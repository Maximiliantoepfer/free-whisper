# Platform and delivery risks

| Risk | Impact | Mitigation | Gate |
| --- | --- | --- | --- |
| Rust toolchain drifts from the pinned version | a local build can use an incompatible compiler | `rust-toolchain.toml` pins 1.97.1; CI reports the selected toolchain | ongoing |
| Node runtime drifts below Vite support | frontend build can warn or fail despite a valid lockfile | pin Node 22.12.0 in `.node-version`, `package.json` and CI | M3.1 |
| MSVC/CMake missing on a release runner | native worker or whisper.cpp cannot be compiled | Windows CI builds the pinned CPU server and worker before every NSIS build | M3.1 |
| WebView2 absent on target machine | desktop UI cannot render | document installer prerequisite and test clean VM | release |
| GPU backend fails | silent CPU fallback would mislead users | CPU is explicit default; GPU is a separately validated package | M4 |
| Elevated target window | Windows input injection can be blocked | refuse paste on an elevated target; copy/confirmation fallback remains visible | M6 implemented; Windows manual gate remains |
| Clipboard changes during paste | user clipboard may be overwritten | sequence counter, text-only conditional restore after 400 ms, explicit warning/outcome | M6 implemented; Windows manual gate remains |
| Remote TLS/auth failure | audio could be sent to wrong endpoint | HTTPS-only, hostname validation, Credential Manager, clear errors | M8 implemented; real TLS gate remains |
| whisper.cpp process exits | request may be lost | health/readiness checks, crash error, controlled restart | M3 |
| Model partial/corruption | invalid inference | signed manifest, SHA-256, atomic install, resume metadata | M4 |
