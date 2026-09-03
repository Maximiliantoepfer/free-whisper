# Meilenstein 3.1 – Qualitäts- und Providerhärtung

**Status:** abgeschlossen am 2026-08-21.

## Geänderte Dateien

- `.node-version`, Root-/Desktop-`package.json`, `pnpm-workspace.yaml` und
  `pnpm-lock.yaml` fixieren Node 22.12.0, pnpm 11.19.0 und
  `@tauri-apps/cli` 2.10.0 am tatsächlichen Tauri-Projektordner.
- `.github/workflows/ci.yml`, `scripts/check.ps1`,
  `scripts/prepare-sidecars.ps1`, `scripts/build-whisper-cpp.ps1` und
  `apps/desktop/src-tauri/tauri.conf.json` bauen und bündeln den CPU-Worker,
  `whisper-server` und dessen notwendigen DLLs in einen NSIS-Installer.
- `scripts/generate-third-party-notices.mjs` und
  `LICENSES/THIRD_PARTY_NOTICES.md` erzeugen und prüfen die deterministische
  Rust-/JavaScript-Attributionsliste aus beiden Lockfiles.
- `crates/protocol`, `crates/domain`, `crates/providers` und `apps/worker`
  erzwingen API-Version 1 auf Erfolgs- und Fehlerantworten, übertragen das
  Audio-Dauerlimit, verwenden typisierte Backendwerte und kapseln Start/Stop
  durch kurze Mutex-Abschnitte und einen separaten Start-Gate.
- `third_party/whisper.cpp` ist als Submodule auf `23ee035` gepinnt. Der
  Adapter fordert `verbose_json` an, übernimmt Segmente/Sprache wenn geliefert
  und misst die Inferenzdauer.
- `apps/worker/src/bin/fake-whisper-server.rs` und
  `apps/worker/tests/worker_process.rs` sind ausschließlich testlokale
  Prozess-Fixtures und werden nicht in das Release-Binary eingebunden.

## Architekturentscheidung

Der lokale Worker bindet selbst an `127.0.0.1:0` und meldet genau einmal seine
tatsächlich gebundene Loopback-Adresse über stdout. Diese Ready-Zeile enthält
kein Token; das Token geht ausschließlich über stdin. Damit reserviert der
Desktop keinen Port mehr vorab. Client und Worker behandeln jede andere
API-Version als sichtbaren Protokollfehler, ohne Downgrade.

Der Windows-Installer enthält nur den expliziten CPU-Server und seine
Laufzeit-DLLs. FFmpeg, Python, PyQt und CTranslate2 sind nicht enthalten.
`whisper.cpp` liefert hier Segmente, aber keine zugesicherten Wortzeitstempel;
der Adapter behauptet diese Fähigkeit daher nicht.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `./scripts/prepare-sidecars.ps1` | bestanden: whisper.cpp `23ee035`, CPU-Server, Rust-Worker und 4 Laufzeit-DLLs gebaut |
| `cargo check --workspace --all-targets` | bestanden |
| `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` | bestanden |
| `cargo test --workspace --features free-whisper-worker/test-fixture` | bestanden: 35 Tests, alle Doc-Tests grün, keine zurückbleibende Fake-Engine |
| `cargo test -p free-whisper-providers` | bestanden: 5 Tests, inklusive API-Mismatch bei Erfolg und Fehler |
| `cargo test -p free-whisper-worker --features test-fixture --test worker_process` | bestanden: echter Workerprozess, tokenfreie Readiness, Busy, Cancel/Restart und verbose JSON |
| `pnpm --dir apps/desktop/ui exec tauri --version` | bestanden: `tauri-cli 2.10.0` |
| `pnpm licenses:generate` / `pnpm licenses:check` | bestanden |
| `pnpm install --frozen-lockfile`, `pnpm ui:check`, `pnpm ui:test`, `pnpm ui:licenses` | bestanden: 0 Svelte-Diagnosen, 1/1 Vitest, Lizenzpolicy grün |
| `cargo deny check --hide-inclusion-graph` / `git diff --check` | bestanden; nur dokumentierte Duplicate-/Workspace-Warnungen |
| `pnpm tauri:build` | bestanden: `target/release/bundle/nsis/free-whisper_2.0.0-alpha.1_x64-setup.exe` (6,079,699 Bytes) |
| NSIS-Inhaltsprüfung | bestanden: Worker, whisper-server, `whisper.dll`, `ggml*.dll` enthalten |

## Offene Risiken

- Die lokale Umgebung besitzt nicht Node 22.12.0. Der Installer wurde trotz
  Vite-Warnung gebaut; die verbindliche CI-Version ist 22.12.0. Ein lokaler
  Release muss vor Veröffentlichung unter dieser Version wiederholt werden.
- Der innere, unversionierte whisper.cpp-Server wählt weiterhin einen freien
  Loopback-Port beim Start. Der stabile Desktop→Worker-Kanal ist race-frei;
  der innere Server bleibt absichtlich nicht nach außen erreichbar und wird
  bei Cancel kontrolliert ersetzt.
