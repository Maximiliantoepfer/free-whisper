# M9 – Legacy-Migration, Benchmark und Release-Abnahme

M9 beginnt erst nach der dokumentierten M4–M6-Hardwareabnahme und einer
erfolgreichen TLS-Abnahme des persönlichen Workers. Es verändert weder die
Legacy-Datenbank noch vorhandene CTranslate2-Modelle.

## 1. Bestätigter Migrationsassistent

| Datei / Komponente | Umsetzung | Abnahme |
| --- | --- | --- |
| `crates/storage/src/legacy_v01.rs` | Öffnet die gewählte v0.1-SQLite-Datei ausschließlich lesend, prüft Tabellen/Spalten und ermittelt einen SHA-256-Quellfingerprint. | Unbekanntes Schema, fehlende Tabellen, defekte Zeitstempel und schreibgeschützte Quelle. |
| `crates/storage/src/lib.rs` | Nutzt eine SQLite-Backup-API in einen vom Nutzer sichtbaren Backup-Pfad und importiert Dictionary/Transkripte in einer Zieltransaktion. | Backup lesbar, Quelle unverändert, Rollback bei jedem fehlerhaften Datensatz. |
| `migration_audit` plus Mappingtabelle | Speichert Fingerprint und v0.1-IDs, damit Wiederholung exakt idempotent ist. | Zweiter Lauf erzeugt keine zusätzlichen Einträge. |
| `apps/desktop/src-tauri/src/lib.rs` | Commands für Vorschau, explizite Bestätigung, Import und Bericht; keine automatische AppData-Suche. | Jeder Command verweigert fehlende Bestätigung. |
| `apps/desktop/ui/src/App.svelte` | Seite „Migration“ mit Quelldatei-Auswahl, Backup-Pfad, Vorschau und getrennten Zählern importiert/übersprungen/fehlerhaft. | Kein automatischer Start, Fehler bleibt sichtbar. |

Legacy-Transkripte erhalten denselben Roh- und Endtext, eine eindeutige
`legacy-v0` Provider-/Modellkennung und keine Audiodatei. Dictionary-Wörter
werden zu globalen Lexikoneinträgen, Kategorie wird übernommen; nicht
interpretierbare QSettings-Werte erscheinen nur in einer bestätigbaren
Vorschau.

## 2. Datenschutzkonformer Benchmark

| Artefakt | Umsetzung | Messung |
| --- | --- | --- |
| `benchmarks/gold/manifest.json` | Versionierte, lizenzierte und dokumentierte Audiodatei-Referenzen; Audio nicht automatisch herunterladen. | Prüfsummen und Lizenzprüfung. |
| `crates/benchmark` | WER/CER, Lexikon-Treffer/Falschkorrekturen, Kalt-/Warmstart, RTF, Ladezeit und Speicherwerte. | Deterministische Tokenisierung und JSON-Messreport. |
| `docs/benchmarks/` | Je Modell/Backend ein datierter Bericht mit Hardwaredaten und Ausreißern. | Keine Qualitätsbehauptung ohne Report. |

## 3. Release-Gate

1. Node 22.12.0, Rust 1.97.1 und ein sauberer Checkout.
2. Stable-Manifestsignatur mit dem ausschließlich in CI vorhandenen Stable-Key;
   Alpha-Key und Alpha-Artefakte dürfen nicht in den Release gelangen.
3. `cargo fmt --check`, Clippy mit `-D warnings`, Workspace-Tests,
   Typecheck/Vitest, Binding-/NOTICE-Drift, `cargo deny` und NSIS-Build.
4. Clean-VM-Installation und M4–M6-Checkliste: Tiny installieren,
   Offline-Aufnahme, Hotkey/Tray, Notepad-Paste samt Clipboard-Race.
5. Optionaler eigener Worker: TLS, Auth, Capability-Mismatch,
   Netzwerkabbruch und Audio-Nichtpersistenz.

Ein veröffentlichtes Windows-x64-Paket braucht außerdem eine manuell geprüfte
`LICENSES/THIRD_PARTY_NOTICES.md`-Sammlung und eine klare SmartScreen-/Code-
Signing-Entscheidung.
