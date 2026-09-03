# Meilenstein 7 – Desktop-Arbeitsoberfläche

**Status:** Implementiert am 2026-08-22; echte Modell-/Mikrofonabnahme aus M4–M6
bleibt ein separates Hardware-Gate.

## Geänderte Dateien

- Der Tauri-Backendvertrag bietet Lexikonprofile, aktive Profile, sichere
  Einträge/Varianten, CSV-/JSON-Import/Export, Verlaufssuche,
  Korrektur-Revert und lokalen Providerstatus als typisierte Commands.
- Die Svelte-Arbeitsfläche enthält nun Lexikon- und Providerseiten sowie
  Verlaufsuche und sichtbare Reverts. Dateiimport und Browserexport übertragen
  lediglich vom Nutzer gewählte Lexikondaten an den Rust-Backendvertrag.
- `scripts/check-desktop-command-bindings.ps1` vergleicht die registrierten
  Rust-Commands mit dem TypeScript-Client und ist in CI und Release eingebunden.

## Architekturentscheidung

Alle Validierung, Unicode-Normalisierung, Importtransaktion, Suche und der
Revert verbleiben in Rust/SQLite. Das Frontend hält nur Formzustand und zeigt
die Rückgaben an. Ein aktives Profil ist für neue Jobs verbindlich, während
globale Einträge weiterhin gelten.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| Storage-/Desktop-Tests | bestanden: Löschkaskade, CSV-Roundtrip und Korrekturpfade |
| `svelte-check` | bestanden: 0 Fehler, 0 Warnungen |
| `bindings:check` | bestanden: 26 Rust-Commands besitzen TypeScript-Bindings |

## Offene Punkte

Die bestätigte Legacy-Migration gehört zu M9. UI-E2E mit echter WebView und
umfangreiche Accessibility-Automation werden nach der Hardwareabnahme ergänzt.
