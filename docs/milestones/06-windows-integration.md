# Meilenstein 6 – Windows-Integration und Collaboration-UI

**Status:** Implementiert am 2026-08-22; die reale Modell-/Mikrofonabnahme
bleibt durch den absichtlich nicht provisionierten Alpha-Signaturschlüssel
blockiert.

## Geänderte Dateien

- `crates/platform-windows/` kapselt Win32 in `native.rs`: eigener
  `RegisterHotKey`-Nachrichtenloop, Vordergrund-Snapshot, Prozessstart-/UAC-
  Prüfung, sequence-counter-geschützte Text-Clipboard-Wiederherstellung und
  `SendInput` für `Ctrl+V`. Die Paste-Policy ist sichere, getestete Rust-Logik.
- `crates/storage/` führt Migration 3 mit opt-in `target_window_json` ein und
  ergänzt atomare sichtbare Clipboard-Warnungen.
- `apps/desktop/src-tauri/` verwendet denselben Aufnahme-/Abbruchcontroller
  für Fensterbutton, Hotkey und Tray. Es stellt Tray-Aktionen, Close-to-tray,
  Sicherheitscommands und typisierte Platform-Events bereit.
- `apps/desktop/ui/` ersetzt das industrielle Raster durch eine ruhigere
  Collaboration-Arbeitsfläche mit Aufnahme, Modellen, Verlauf sowie Windows-
  und Sicherheitsseite.
- `resources/brand/free-whisper-signal-master.png` ist die eigenständige,
  generierte v2-Bildmarke. Die daraus von Tauri erzeugten ICO-/PNG-Varianten
  unter `apps/desktop/src-tauri/icons/` sind in Bundle, Fenster und Tray
  referenziert.
- README, Architektur, Privacy, Command-Vertrag, Risikoübersicht und ADR 0008
  dokumentieren den tatsächlichen Sicherheits- und Datenfluss.

## Architekturentscheidung

Bestätigen/Copy ist Standard; Auto-Paste ist standardmäßig deaktiviert. Es
erfolgt nur bei exakt identischem HWND, PID und Prozessstartzeit, nie in ein
erhöhtes Ziel. Bei jeder Unsicherheit wird nicht eingefügt. Der Text bleibt
kopiert, Ergebnis und Warnung sind in der Historie sichtbar.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `cargo check -p free-whisper-platform-windows` | bestanden |
| `cargo fmt --all -- --check` | bestanden |
| `cargo clippy --workspace --all-targets -- -D warnings` | bestanden |
| `cargo test --workspace --features free-whisper-worker/test-fixture --quiet` | bestanden: 48 Tests, einschließlich Windows-Paste-Policy und atomarer Clipboard-Warnung |
| Svelte Check | bestanden: 0 Fehler, 0 Warnungen |
| Vitest | bestanden: 1 Testdatei, 1 Test |
| `pnpm ui:licenses` / `pnpm licenses:check` | bestanden |
| `cargo deny check --hide-inclusion-graph` | bestanden; nur dokumentierte Duplicate-/Workspace-Warnungen |
| `git diff --check` | bestanden |
| `pnpm tauri:build` | bestanden: Windows-x64-NSIS, 8,362,963 Bytes, SHA-256 `8E9AC4E7A8E8544E14EC8F5BA34E60AE220E1AFC0C0A4242AEFC67267FF56245` |

Der Build verwendete innerhalb von Tauri die lokal verfügbare Node 20.18.0 und
gab die bekannte Vite-Warnung aus (mindestens 20.19 / Ziel 22.12). CI bleibt auf
22.12.0 gepinnt; diese lokale Installation muss vor einem signierten Release
korrigiert werden.

## Offene Risiken und nächste Schritte

- `FREE_WHISPER_MANIFEST_SIGNING_KEY` ist in dieser Umgebung nicht vorhanden.
  Deshalb ist Tiny-Download/Hashprüfung/echte Mikrofon- und Notepad-Abnahme
  korrekt blockiert; es wurde kein Testschlüssel eingecheckt.
- Die M5.1-Warteschlange ist nun an den Desktopcontroller gebunden: eine
  Inferenz plus genau eine finalisierte, begrenzte Aufnahme können warten;
  weiterer Überlauf liefert `queue_full`. Die reale Hardwareabnahme muss den
  Warten-/Abbruchpfad nach Signaturfreigabe zusätzlich verifizieren.
- Notwendige manuelle Windows-Fälle nach Signaturfreigabe: Hotkey-Kollision,
  Notepad unverändert/gewechselt, erhöhtes Ziel, Clipboard-Race, Tray-Quit und
  Sidecar-Shutdown.
- M7 folgt mit vollständigen Lexikon-/Provider-Einstellungen, Binding-
  Driftprüfung, UI-E2E und Accessibility-Smoke-Tests; M8/M9 bleiben Remote-
  Secret Store beziehungsweise bestätigte Legacy-Migration und Benchmark.
