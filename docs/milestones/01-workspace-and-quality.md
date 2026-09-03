# Meilenstein 1 – Workspace und Qualitätsbasis

**Status:** abgeschlossen am 2026-08-21.

## Geänderte Dateien

- Die vollständige Python/PyQt-Referenz liegt jetzt unter `old/python/`:
  `src/`, `assets/`, Python-Einstiegspunkte, PyInstaller-Dateien,
  Anforderungen, Hooks, das alte README und lokale Claude-Metadaten. Die
  generierten, ausschließlich repository-lokalen Verzeichnisse `build/`,
  `dist/` und `.venv/` wurden nach aufgelöster Pfadprüfung entfernt.
- Der Repository-Root enthält nun den Rust-Workspace (`Cargo.toml`,
  `Cargo.lock`, `rust-toolchain.toml`) mit Domain, Application, Storage,
  Lexicon, Providers, Audio, Windows-Plattform, Protocol und Test-Support sowie
  Tauri-Desktop und Worker.
- `crates/domain/src/lib.rs` enthält starke IDs, Auftragsdaten, Provider- und
  Ergebnis-Typen und die jobgebundene, getestete Zustandsmaschine.
- `crates/application/src/lib.rs` enthält die Kapazität-1-Warteschlange und
  den jobgebundenen Zustands-Registry-Schutz. `test-support` stellt einen
  deterministischen Fake-Provider bereit.
- `apps/desktop/` enthält einen startbaren Tauri-2-/Svelte-Grundaufbau. Die
  sichtbaren Statusnamen und ihre Frontend-Tests sind absichtlich nur
  Darstellung; Geschäftslogik bleibt im Rust-Backend.
- `.github/workflows/ci.yml`, `scripts/check.ps1`, `deny.toml` und
  `scripts/check-js-licenses.mjs` bilden die einheitliche Qualitäts- und
  Lizenzprüfung. `LICENSES/THIRD_PARTY_NOTICES.md` dokumentiert das Verfahren.

## Architekturentscheidung

Der neue Workspace besitzt keine Python-Laufzeitabhängigkeit und keine
Übernahme der Legacy-Architektur. Zustandsänderungen gehören zu genau einer
`JobId`; ein anderer Job kann ihren Zustand nicht verändern. Die Arbeitsqueue
speichert höchstens einen wartenden Job und meldet Überlauf explizit, statt
Audiodaten still zu verlieren. Der lokale Worker ist vorerst nur ein
Prozess-Skelett und wird in Meilenstein 3 durch den realen, versionierten
Providervertrag ersetzt.

Die anfängliche Tauri-ICO-Datei ist lediglich eine kopierte Legacy-Bildressource
für einen funktionierenden Windows-Build; kein Python- oder PyQt-Bestandteil
wird ausgeliefert. Sie wird vor dem Release durch ein v2-eigenes Asset ersetzt.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `cargo fmt --all -- --check` | bestanden |
| `cargo clippy --workspace --all-targets -- -D warnings` | bestanden |
| `cargo test --workspace` | bestanden: 6 Unit-Tests, alle Doc-Tests grün |
| `pnpm ui:check` | bestanden: 0 Fehler, 0 Warnungen |
| `pnpm ui:test` | bestanden: 1/1 Vitest-Test |
| `pnpm ui:licenses` | bestanden: Apache-2.0, Apache-2.0 OR MIT, BSD-3-Clause, ISC, MIT |
| `cargo deny check --hide-inclusion-graph` | bestanden: Advisories, Bans, Licenses und Sources grün; nur dokumentierte Versions-/Workspace-Warnungen |
| `git diff --check` | bestanden |

## Offene Risiken

- Die aktuellen Tauri-Abhängigkeiten bringen mehrere als *unmaintained*
  markierte, transitive `urlpattern`-/Unicode-Helfer mit. Fünf konkret
  dokumentierte RustSec-Einträge sind bis zu einem sicheren Upstream-Update
  überprüfungspflichtige Ausnahmen, keine still unterdrückten Befunde.
- Audio, SQLite, Modelldownload, whisper.cpp-Ausführung, Windows-Integration
  und die fachlichen UI-Seiten sind noch nicht implementiert; sie sind die
  folgenden Meilensteine und keine produktiven Platzhalter.
- Ein `git mv` konnte wegen schreibgeschütztem Git-Index in dieser Umgebung
  nicht verwendet werden. Die Inhalte wurden nach vorheriger Pfadprüfung
  direkt verschoben; Git erkennt sie beim späteren Commit als Umbenennung.
