# Meilenstein 5.2.1 – Sidecar-Freigabe und Aufnahmehärtung

**Status:** Implementiert am 2026-09-03; physische Mikrofon-/Notepad-Abnahme
steht weiterhin aus.

## Geänderte Bereiche

- Der lokale Worker besitzt einen tokenfreien `--build-info`-Modus. Desktop,
  Sidecar-Prepare-Skript und Tauri-Preflight verlangen dieselbe Paketversion,
  Protokollversion und den fest gepinnten whisper.cpp-v1.8.6-
  `verbose_json`-Adapter.
- Root-`tauri:dev` und `tauri:build` bereiten Sidecars nun selbst vor. Direkte
  Tauri-Aufrufe prüfen nur und nennen die korrekte Root-Aktion bei fehlenden
  oder alten Artefakten.
- `ProviderError::Protocol` und eine veraltete Sidecar-EXE werden in sichere,
  handlungsorientierte Desktop-Fehler übersetzt. Technische Details verbleiben
  in lokalen Diagnoseausgaben, niemals im UI-Text.
- Die Svelte-DTOs enthalten Backend-Phase, aktive Job-ID und monotone
  Ereignisnummer. Push-to-talk merkt sich ein Loslassen während der
  Vorbereitung; Stop-Anfragen von Button, VAD und PTT werden pro Ablauf
  zusammengeführt.

## Architekturentscheidung

Kompilierte Sidecars bleiben Git-ignorierte Buildprodukte. Ihre Identität wird
zur Laufzeit und beim Packaging verifiziert, statt Binärdateien in Git zu
versionieren. Die Rust-Zustandsmaschine bleibt alleinige Autorität; das
Frontend behandelt ausschließlich Eingabereihenfolge und veraltete Events.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `scripts/prepare-sidecars.ps1` / `verify-sidecars.ps1` | bestanden: gestagte Worker-EXE meldet `2.0.0-alpha.2`, API v1 und `whispercpp-v1.8.6-verbose-json-start-end` |
| `cargo fmt --all -- --check` | bestanden |
| `cargo clippy --workspace --all-targets -- -D warnings` | bestanden |
| `cargo test --workspace --features free-whisper-worker/test-fixture` | bestanden: 70 Tests einschließlich Worker-Prozess, Build-Info, Adapter-Fixture, Coordinator und Desktop-Fehlerzuordnung |
| `pnpm ui:check` / `pnpm ui:test` | bestanden: 0 Svelte-Diagnosen, 5 Vitest-Tests |
| `pnpm licenses:check` / `cargo deny check --hide-inclusion-graph` | bestanden; nur dokumentierte Duplicate-/Workspace-Warnungen |
| `pnpm tauri:build` | bestanden: `target/release/bundle/nsis/free-whisper_2.0.0-alpha.2_x64-setup.exe` |

## Offene Abnahme

Die physische Windows-Abnahme mit Tiny-Modell, Mikrofon und Notepad bleibt
erforderlich: Toggle, kurzes und normales Push-to-talk, globaler Hotkey,
erneute Aufnahme nach Fehler sowie Copy/Paste-Fallback. Der neue
Alpha.2-Installer ist hierfür zwingend zu verwenden; Alpha.1 enthält den alten
Worker und ist kein gültiger Teststand mehr.
