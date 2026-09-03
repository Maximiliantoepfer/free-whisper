# Meilenstein 4.2 – robuster Modellpfad und Einrichtungs-Gate

**Status:** Implementiert am 2026-08-22; die physische Tiny-/Mikrofonabnahme
bleibt eine manuelle Windows-Prüfung.

## Geänderte Bereiche

- `crates/model-manager/src/lib.rs`: atomare Aktivierung für namespacete
  Modell-IDs, sichere Pfadsegmente, Windows-kompatibles Schließen des
  Download-Handles, Wiederherstellung der betroffenen Alpha-Staging-Datei
  sowie Regressionstests mit einem echten lokalen HTTP-Download.
- `apps/desktop/src-tauri/src/lib.rs`: backenddefinierter `OnboardingState`
  und ein strukturierter, nutzerfreundlicher `model_storage_failed`-Fehler.
- `apps/desktop/ui/`: separater Einrichtungsraum für lokales Modell oder
  eigenen Remote-Worker; die vollständige Navigation erscheint erst danach.

## Architekturentscheidung

Modell-IDs bleiben fachliche, namespacete IDs wie `whisper.cpp/tiny`. Ihre
Speicherpfade werden jedoch Segment für Segment unter dem konfigurierten
Modellordner aufgebaut. Temporäre Installationsordner sind zufällige,
benachbarte Namen ohne Modell-ID. Damit bleibt der abschließende Rename
atomar, funktioniert unter Windows und kann keinen Pfad außerhalb des
Modellordners ansprechen.

Eine eng begrenzte Kompatibilitätsroutine erkennt genau den fehlerhaften
Staging-Pfad der vorherigen Alpha-Version und übernimmt dessen einzelne,
durch Resume-Metadaten belegte Datei wieder in den normalen Prüfpfad. Der
SHA-256-Check bleibt auch für diese Wiederherstellung zwingend.

Der Desktop leitet seinen sichtbaren Modus ausschließlich aus dem typisierten
Backend-Feld `onboarding` ab. UI-Navigation ist keine Sicherheitsgrenze:
Aufnahme-Commands prüfen weiterhin aktivierten Provider und Modell.

## Ausgeführte Prüfungen

- `cargo test -p free-whisper-model-manager -p free-whisper-desktop` — grün
  (13 Tests, darunter verschachtelter Windows-Modellpfad und Fehlerabbildung).
- `cargo test --workspace` — grün (58 Tests).
- `cargo fmt --all -- --check` und `cargo clippy --workspace --all-targets -- -D warnings` — grün.
- `corepack pnpm --dir apps/desktop/ui check` — grün.
- `corepack pnpm --dir apps/desktop/ui test:run` — grün (2 Tests).
- `corepack pnpm --dir apps/desktop/ui build` — grün.
- `scripts/check-desktop-command-bindings.ps1` — grün (32 Commands).
- `corepack pnpm licenses:check` und `cargo deny check --hide-inclusion-graph` — grün;
  nur die bereits dokumentierten Workspace-/Dublettenwarnungen.
- `git diff --check` — grün.
- `corepack pnpm tauri:build` — grün; NSIS-Installer erstellt.

## Offene Risiken und manuelle Abnahme

- Mit dem realen, signierten Tiny-Artefakt prüfen: Installieren & aktivieren,
  Abbruch, Wiederaufnahme und anschließende lokale Transkription.
- Windows-Mikrofon, Hotkey, Tray und sicheres Paste bleiben hardwareabhängige
  M5/M6-Abnahmen und werden nicht durch die lokale HTTP-Fixture ersetzt.
- Die verwendete Umgebung hat Node 24.19; für CI und Releases bleibt Node
  22.12 verbindlich.
