# Meilenstein 3 – Providervertrag und Worker

**Status:** abgeschlossen am 2026-08-21.

## Geänderte Dateien

- `crates/protocol/src/lib.rs` enthält den stabilen, versionierten HTTP-
  Vertrag v1 mit DTOs für Health, Capabilities, Modelle, Multipart-
  Transkription, Abbruch und strukturierte Fehler.
- `apps/worker/src/lib.rs` implementiert den Axum-Worker mit Authentisierung,
  Größen-/Dauer-/Rate-/Parallelitätsgrenzen, WAV-/PCM-Validierung und einem
  gekapselten whisper.cpp-Prozessadapter.
- `apps/worker/src/main.rs` startet den Worker ausschließlich auf Loopback,
  nimmt das Bearer-Token nur über stdin entgegen und verweigert nicht-loopback
  HTTP.
- `crates/providers/src/lib.rs` implementiert den HTTPS-Remote-Client und den
  lokalen Sidecar-Manager mit ephemerem Port und ausschließlich im Speicher
  gehaltenem stdin-Token.
- `docs/remote-provider-protocol.md`, `docs/license-inventory.md`,
  `deny.toml` und die Cargo-Manifeste dokumentieren das Protokoll sowie die
  zusätzlich geprüften TLS-Lizenzen.

## Architekturentscheidung

Die Desktop-App spricht nie direkt mit whisper.cpp. Der Worker ist die einzige
stabile v1-Grenze und hält whisper.cpp als separaten, loopback-gebundenen
inneren Prozess. PCM-f32-Mono-Audio wird vor der Engine rein im Speicher in WAV
verpackt. Ein Cancel signalisiert den aktiven Job, beendet den inneren Prozess
kontrolliert und startet ihn erneut; er wird nicht still ignoriert.

Remote-Profile verlangen HTTPS. Nur ein ausdrücklich aktivierter
Entwicklungsmodus darf HTTP zu einer nachweislichen Loopback-Adresse nutzen.
Der lokale Provider erzeugt Port und Token je Start neu und schreibt den Token
nur in den stdin-Kanal des Kindprozesses. Weder Token noch Audio haben einen
SQLite- oder Logpfad.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `cargo fmt --all -- --check` | bestanden |
| `cargo clippy --workspace --all-targets -- -D warnings` | bestanden |
| `cargo test --workspace` | bestanden: 25 Unit-Tests, alle Doc-Tests grün |
| Worker-/Provider-Contract-Test | bestanden: echter Loopback-Axum-Worker, Auth, Capabilities, Modellprüfung und Multipart-PCM-Request |
| Worker-Grenztests | bestanden: fehlende Authentisierung, nicht-finite PCM-Samples, Dauerlimit und PCM→WAV-Adapter |
| `pnpm ui:check` / `pnpm ui:test` | bestanden: 0 Diagnosefehler, 1/1 Test |
| `cargo deny check --hide-inclusion-graph` | bestanden; neue ISC- und CDLA-Permissive-2.0-Abhängigkeiten explizit dokumentiert |
| `git diff --check` | bestanden |

## Offene Risiken

- Der echte whisper.cpp-Binary- und Modell-Download wird erst durch den
  ModelManager in Meilenstein 4 installiert und paketiert. Der Prozessadapter
  ist implementiert, aber dieser Meilenstein führt keine Inferenz gegen ein
  externes Modell aus.
- Der Worker akzeptiert absichtlich nur Loopback-HTTP. TLS-Zertifikate,
  Reverse-Proxy-Anleitung und Credential-Manager-Integration folgen im
  Remote-Betriebsmeilenstein; die Client-Seite erzwingt HTTPS schon jetzt.
- Die ephemeral-port-Reservierung zwischen Freigabe und Sidecar-Start besitzt
  das übliche lokale Race-Fenster. Authentifizierte Readiness verhindert eine
  Verwechslung; ein späterer Sidecar-Transport kann dies weiter härten.
