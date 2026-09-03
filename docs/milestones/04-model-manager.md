# Meilenstein 4 – Signierter ModelManager

**Status:** Alpha-Katalog freigeschaltet am 2026-08-22; echte Download- und
Hardwareabnahme steht noch aus.

## Geänderte Dateien

- `crates/model-manager/` implementiert RFC-8785-Kanonisierung, Ed25519-
  Verifikation, typisierte CPU/GPU-Backendwerte, HTTPS-only Download,
  Range-/ETag-Resume, Durchsatz/ETA, Abbruch, Größen-/SHA-256-Prüfung,
  Staging-Aktivierung und bestätigte Löschung.
- `resources/models/manifest.json` beschreibt die CPU-Presets tiny, base und
  small mit fester Hugging-Face-Revision, Lizenz/Provenienz, Größe und SHA-256.
  Der kanonisch signierte `trust_scope: alpha` trennt diesen Katalog von
  späteren Stable-Artefakten.
- `apps/desktop/src-tauri/src/lib.rs` stellt explizite Backend-Commands für
  Katalog, Installieren, Abbrechen und bestätigtes Löschen bereit. Der Command
  persistiert nur validierte Installationsmetadaten in SQLite.
- `crates/storage/src/lib.rs` kann Installationsmetadaten beim bestätigten
  Löschen entfernen. `docs/model-manifest.md`, Architektur, Lizenzinventar,
  Release-Workflow und der Folgeplan dokumentieren den Ablauf.

## Architekturentscheidung

Kein Aufnahmepfad kann einen Download auslösen. Der ModelManager akzeptiert
einen Katalog nur nach Signaturprüfung und startet erst danach einen
HTTPS-Download. Teilstände erhalten URL-, Größen-, Hash- und ETag-Bindung;
aktiv wird ein Modell erst nach vollständiger Hashprüfung und atomarem Rename.
GPU-Katalogeinträge werden im CPU-Release explizit abgewiesen, nicht auf CPU
umgebogen.

Die Alpha-Signatur ist kein Stable-Release-Fallback. Scope und Signatur gehören
zum selben kanonischen Payload; ein Stable-Binary verweigert daher jeden
`alpha`-Katalog, auch wenn dessen Signatur formal gültig ist.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `cargo test -p free-whisper-model-manager` | bestanden: 8 Tests für Signatur/Tampering, Alpha-/Stable-Trennung, HTTPS-/Backend-Validierung, korruptes Modell, Abbruch, Resume-ETag, atomaren Ziel-Race und Löschen |
| `cargo check -p free-whisper-desktop` | bestanden: Tauri-Commands und SQLite-Metadatenpfad |
| `cargo test -p free-whisper-providers` | bestanden |
| vollständiges Rust-Gate | bestanden: `cargo fmt --all -- --check`, Clippy mit `-D warnings` und 35 Workspace-Tests inklusive Prozessfixture |
| Frontend-/Lizenz-/Diff-Gate | bestanden: gefrorene pnpm-Installation, Svelte-Check, Vitest, NOTICE-Drift, cargo-deny und `git diff --check` |
| `pnpm tauri:build` | bestanden: Windows-x64-NSIS-Installer mit CPU-Sidecars |
| Katalog-Integrität in CI/Release | eingerichtet: CI verifiziert den eingecheckten Alpha-Katalog; Release signiert und verifiziert ausschließlich den Stable-Katalog |

## Blocker und offene Risiken

- **Offene Abnahme:** Tiny muss über die Desktop-App bewusst heruntergeladen,
  gehasht und aktiviert werden. Die lokale Alpha-Identität ist kein
  Stable-Release-Schlüssel; Stable CI schreibt einen getrennten Scope und Key.
- Ein vollständiger HTTPS-Resume-Integrationstest mit lokaler TLS-Test-CA muss
  nach Provisionierung der Signatur ergänzt werden. Die Unit-Tests prüfen
  HTTPS-Zwang, ETag-Zustand, Hash, Größe, Abbruch und Aktivierung bereits
  deterministisch.
- Lokales Node ist 20.18.0/24.19.0 statt 22.12.0 je aufgerufener Shell. Die
  CI- und Release-Workflows pinnen 22.12.0; die lokale Release-Abnahme muss mit
  dieser Version wiederholt werden.
