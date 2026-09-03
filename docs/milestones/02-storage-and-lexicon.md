# Meilenstein 2 – SQLite und Lexikon

**Status:** abgeschlossen am 2026-08-21.

## Geänderte Dateien

- `crates/storage/src/lib.rs` implementiert die atomare Schema-Migration v1
  sowie Repositories für Settings, Providerprofile, installierte Modelle,
  Lexikonprofile/-einträge/-varianten, Transkripte, Korrekturen und
  Migrationsaudit.
- `crates/lexicon/src/lib.rs` implementiert NFC-Normalisierung,
  casefold-basierte Identitäten, deterministische Prompt-Auswahl und sichere
  Variantenkorrektur samt Revert-Renderer.
- `crates/domain/src/lib.rs` enthält nun eine eigene
  `TranscriptCorrectionId`, damit einzelne Korrekturen persistent und
  nachvollziehbar rückgängig gemacht werden können.
- `Cargo.toml`, die Crate-Manifeste und `Cargo.lock` ergänzen SQLite, CSV und
  Unicode-Abhängigkeiten. `docs/architecture.md` und `docs/privacy.md`
  dokumentieren die Persistenz- und Secret-Grenzen.

## Architekturentscheidung

SQLite ist die alleinige Quelle persistenter App-Daten. Die Datenbank aktiviert
Foreign Keys, deaktiviert `trusted_schema` und führt jede versionierte Migration
in einer `IMMEDIATE`-Transaktion aus. Audio und Zugangstokens haben keinen
Speicherpfad: Providerprofile enthalten ausschließlich eine Credential-Manager-
Referenz und einen typisierten, nicht geheimen Verbindungssatz. Das Settings-
Repository weist sensible Schlüssel und verschachtelte sensible JSON-Felder vor
dem Schreiben ab.

Lexikoneinträge werden als NFC-Text mit casefold-basierter Identität gespeichert.
Prompts enthalten höchstens 32 deterministisch ausgewählte Begriffe mit einem
konservativen 128-Token-Schätzwert. Nachkorrekturen sind nur explizite Varianten
an Unicode-Wortgrenzen; weder Fuzzy-Matching noch Regex ist vorhanden. Jede
Korrektur bezieht sich auf den unveränderten Rohtext. Ein Revert markiert genau
eine Korrektur als zurückgenommen und berechnet `final_text` aus den übrigen
Regeln vollständig neu.

CSV- und JSON-Import werden vor der Datenbanktransaktion vollständig geparst
und validiert. Der Bericht unterscheidet hinzugefügte Einträge/Varianten,
Duplikate und übersprungene Einträge; Validierungsfehler werden strukturiert
zurückgegeben, ohne Teilimport. Die Transkriptsuche verwendet ausschließlich
gebundene, literal escapte `LIKE`-Parameter, nicht unkontrollierte FTS-Syntax.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `cargo fmt --all -- --check` | bestanden |
| `cargo clippy --workspace --all-targets -- -D warnings` | bestanden |
| `cargo test --workspace` | bestanden: 18 Unit-Tests, alle Doc-Tests grün |
| Storage-/Lexikon-Integrationstests | bestanden: Migration, Secretschutz, CSV/JSON-Import, Duplikate, Suche und Revert |
| `pnpm ui:check` | bestanden: 0 Fehler, 0 Warnungen |
| `pnpm ui:test` | bestanden: 1/1 Test |
| `pnpm ui:licenses` | bestanden |
| `cargo deny check --hide-inclusion-graph` | bestanden; nur dokumentierte Versions-/Workspace-Warnungen |
| `git diff --check` | bestanden |

## Offene Risiken

- Die Laufzeit verwendet derzeit `LIKE` statt einer FTS-Tabelle. Das ist sicher
  und fehlertransparent, aber bei sehr großen Transkripthistorien langsamer; ein
  späterer FTS-Ausbau braucht eigene atomare Migrationen und Fehleroberfläche.
- Der Legacy-Import selbst folgt erst in Meilenstein 9. Das vorhandene
  `migration_audit` liefert bereits die idempotente Grundlage, importiert aber
  noch keine v0.1-Daten.
- Kein Audio-, Provider- oder Modelldownloadpfad existiert bisher. Persistierte
  Modell- und Providerdatensätze sind absichtlich nur die sichere Grundlage für
  die folgenden Meilensteine.
