# Meilenstein 5 – lokale Transkription: vertikaler Alpha-Slice

**Status:** Implementiert am 2026-08-22; Freigabe weiterhin vom signierten
Alpha-Modellkatalog abhängig.

## Geänderte Dateien

- `crates/audio/` implementiert CPAL-Geräteauswahl, vorallokierte begrenzte
  Aufnahme, Downmix, Rubato-Resampling auf 16-kHz-Mono-PCM-f32 sowie den
  WebRTC-Pfad von `wavekat-vad`. Geräteverlust, Callback-Überlauf,
  VAD-Fehler, leere Aufnahme und Längenlimit sind explizite Fehler.
- `crates/storage/` migriert Transkripte atomar auf Ergebnismetadaten,
  Segmente, Warnungen und detaillierte Laufzeiten. Die Aktivierung lokaler
  CPU-Modelle ist atomar und lässt genau ein validiertes aktives Modell zu.
- `apps/desktop/src-tauri/` enthält den backendseitigen Einzeljob-Controller:
  Sidecar-Readiness vor der Aufnahme, gebundene Zustandsübergänge,
  Lexikon-Snapshot, lokale Provider-Inferenz, Speicherung und explizites
  Clipboard-Kopieren. Alle Command-/Event-DTOs sind Rust-definiert.
- `crates/platform-windows/` schreibt ausschließlich auf einen expliziten
  Nutzerklick Text in die Zwischenablage. Kein Auto-Paste und keine
  Clipboard-Wiederherstellung wurden eingebaut.
- `apps/desktop/ui/` ersetzt die statische Seite durch den keyboard-bedienbaren
  „Local Signal Desk“: Modelle mit ehrlichen Metadaten und Fortschritt,
  Mikrofon-/VAD-/Zeitlimitsteuerung, Push-to-talk/Toggle, Pegel,
  Ergebnisansicht und Verlauf.
- `deny.toml`, `docs/license-inventory.md` und
  `LICENSES/THIRD_PARTY_NOTICES.md` erfassen die zusätzliche BSL-1.0-
  Clipboard-Abhängigkeit nachvollziehbar.
- `docs/adr/0007-audio-vad.md`, Architektur-, Privacy-, Command- und
  README-Dokumentation beschreiben den tatsächlichen Datenfluss.

## Architekturentscheidung

Die UI ist ausschließlich ein typisierter Tauri-Client. Der Desktop-Backend
besitzt Aufnahme, Worker, Zustandsmaschine, SQLite und Clipboard-Adapter. Ein
Job darf nur seinen eigenen Processing-Eintrag auflösen; ein Aufräumfehler des
lokalen Workers wird an die UI zurückgegeben statt verschluckt.

Das Live-VAD und der Vorabcheck verwenden dieselben WebRTC-Parameter. Audio
bleibt als begrenzter RAM-Puffer auf dem Gerät, wird nach dem Provider-Aufruf
freigegeben und nie als Datei gespeichert. Der Resultattext wird nur nach
Klick kopiert und die Outcome-Änderung wird in SQLite abgelegt.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `cargo fmt --all -- --check` | bestanden |
| `cargo clippy --workspace --all-targets -- -D warnings` | bestanden |
| `cargo test --workspace --quiet` | bestanden: 43 Tests, einschließlich Audio-Resampling, VAD-Stille, Stereo-Downmix/Buffergrenze, Geräte-/Overflow-Fehler, Modellvalidierung, SQLite- und Worker-Prozess-Tests |
| `pnpm ui:check` / `pnpm ui:test` | bestanden: 0 Svelte-Diagnosen, 1 Vitest-Datei grün |
| `pnpm ui:licenses` / `pnpm licenses:check` | bestanden; deterministische NOTICE-Datei aktuell |
| `cargo deny check --hide-inclusion-graph` | bestanden; nur dokumentierte Duplicate-/Workspace-Warnungen |
| `git diff --check` | bestanden |
| `pnpm tauri:build` | bestanden: `target/release/bundle/nsis/free-whisper_2.0.0-alpha.1_x64-setup.exe` (6,636,597 Bytes) |

## Offene Risiken und nächste Schritte

- **Release-Blocker:** Die eingecheckten Modell-Signaturdateien sind weiterhin
  sichere Sentinels. Erst ein geschütztes
  `FREE_WHISPER_MANIFEST_SIGNING_KEY` kann den Katalog freigeben; ohne ihn
  sind Modellinstallation und eine physische End-to-End-Transkription bewusst
  nicht testbar.
- Die lokale Toolchain erfüllt Node 22.12.0 nicht konsistent: pnpm meldet
  24.19.0, Vite läuft mit 20.18.0 und warnt, obwohl der Build gelang. CI ist
  auf 22.12.0 gepinnt; die Release-Abnahme muss dort wiederholt werden.
- Der Slice erlaubt höchstens einen Live-Job und liefert `queue_full`, statt
  Audio für einen wartenden Job zu halten. Der kapazitäts-eins Coordinator mit
  zweitem, begrenztem Job gehört in die nächste Orchestrierungsiteration.
- Globaler Hotkey, Tray, Auto-Paste, Remote-Secret-Store und
  Legacy-Migrationsassistent bleiben absichtlich außerhalb dieses Slice.
