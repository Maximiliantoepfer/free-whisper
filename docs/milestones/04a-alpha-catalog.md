# Meilenstein 4.1 – signierter Alpha-Modellkatalog

**Status:** Implementiert am 2026-08-22; reale Download- und
Hardwareabnahme stehen noch aus.

## Geänderte Dateien

- `crates/model-manager/` führt den signierten `trust_scope` mit getrennten
  Alpha-/Stable-Prüfpfaden ein sowie ein schlüsselloses Verifikationsprogramm.
- `scripts/provision-alpha-manifest.ps1` erzeugt bei Bedarf einen zufälligen
  lokalen Alpha-Seed in der ignorierten `.env.alpha.local` und erzeugt daraus
  die eingecheckte Signatur samt öffentlichem Key.
- Die Desktop-Buildlogik leitet den erwarteten Scope aus der Paketversion ab;
  der Stable-Release-Workflow erzeugt in seinem sauberen Checkout zwingend ein
  `stable`-Manifest.

## Architekturentscheidung

Der private Alpha-Seed wird nie eingecheckt. Die öffentliche Alpha-Identität
ist für nachvollziehbare Vorab-Builds zulässig, aber kryptografisch nicht für
einen Stable-Build verwendbar: Scope und Signatur gehören zum selben kanonischen
Payload und ein Stable-Binary verweigert `alpha`.

## Ausgeführte Prüfungen

| Prüfung | Ergebnis |
| --- | --- |
| `.env.alpha.local` | erzeugt und durch Git-Ignorierregel bestätigt, ohne den Seed zu lesen |
| `verify-model-manifest … --scope alpha` | bestanden |
| `cargo test -p free-whisper-model-manager --quiet` | bestanden: 8 Tests, einschließlich Scope-Mismatch |

## Offene Abnahme

Tiny muss noch bewusst aus der Desktop-App heruntergeladen werden. Erst die
erfolgreiche Hash-/Aktivierungsprüfung und die reale Mikrofontranskription
schließen M4 und M5 ab.
