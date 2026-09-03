# Meilenstein 8 – Persönlicher Remote-Betrieb

**Status:** Implementiert am 2026-08-22; reale TLS-/Worker-Abnahme bleibt ein
explizites Hardware- und Netzwerk-Gate.

## Gelieferter Ablauf

1. Die Provider-Seite testet einen eingegebenen Endpoint, Bearer-Token,
   HTTPS-/Loopback-Regel, Health, Capabilities und verfügbare Modelle, ohne
   dabei Daten zu speichern.
2. Erst **Speichern & aktivieren** legt ein Profil mit Endpoint, Zeitlimits und
   ausgewähltem Worker-Modell in SQLite ab. Das Token wandert ausschließlich in
   den Windows Credential Manager unter einem app-eigenen, undurchsichtigen
   Namen.
3. Der jobgebundene Aufnahme-Coordinator verwendet den aktivierten
   `RemoteWorkerProvider` mit denselben VAD-, Queue-, Abbruch-, Lexikon- und
   Ergebnisregeln wie der lokale Provider. Lokaler Provider bleibt Standard
   und lässt sich jederzeit bewusst zurückschalten.

## Sicherheitsentscheidungen

- Eine Remote-Verbindung darf nicht durch einen gespeicherten URL-Wert allein
  aktiv werden: Sie braucht eine existierende Credential-Referenz, ein
  aktiviertes Profil und ein ausgewähltes, bereites Modell.
- Der Desktop besitzt keine Remote-Prozesskontrolle. Abbruch erfolgt über das
  versionierte API; Cleanup beendet niemals den fremden Worker.
- Token, Audio und Transkripttext werden nicht in Tauri-Events, SQLite oder
  Standardlogs ausgegeben.

## Automatische Prüfung

- Credential-Referenz- und Token-Grenzen, Remote-Timeout-Grenzen,
  HTTP-Loopback-Regel sowie die Version-/Contract-Tests des Workers sind
  unit-getestet.
- Der komplette Workspace-, Format-, Clippy-, Typecheck-, Binding-, Lizenz- und
  Windows-NSIS-Build-Gate wird nach dieser Lieferung ausgeführt.

## Verbleibende Abnahme

Eine reale Worker-Instanz hinter einer vertrauenswürdigen TLS-Konfiguration
muss noch manuell getestet werden: ungültiges Zertifikat, fehlender Token,
Capability-Mismatch, Netzwerkabbruch und eine erfolgreiche Aufnahme. Das
verbleibt absichtlich sichtbar, weil ein lokaler Test keine fremde
Netzwerk-/Zertifikatskette ersetzen kann.
