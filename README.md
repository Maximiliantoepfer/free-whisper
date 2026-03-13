# free-whisper

OpenSource Project for local and cloud based whisper based speech-to-text platform.

---

## Setup (Development)

**Voraussetzungen:** Python 3.10+

```
python -m venv .venv
.venv\Scripts\activate
pip install -e .
```

`pip install -e .` installiert alle Runtime-Abhängigkeiten aus `pyproject.toml`.
Für reproduzierbare Builds mit gepinnten Versionen stattdessen:

```
pip install -r requirements.txt && pip install -e . --no-deps
```

App starten:

```
free-whisper
```

---

## Als .exe kompilieren (Windows)

**Voraussetzungen:** Python 3.10+, PyInstaller

```
pip install -r requirements-dev.txt
```

Build ausführen:

```
python -m PyInstaller free-whisper.spec
```

Die fertige EXE liegt unter `dist\free-whisper\free-whisper.exe` (Ordner-Build, keine einzelne Datei).

### Debug- vs. Release-Build

In `free-whisper.spec` im `EXE()`-Block die Option `console` anpassen:

| Variante | Einstellung | Beschreibung |
|----------|-------------|--------------|
| **Debug** | `console=True` | Konsolenfenster bleibt offen — zeigt Logs und Fehlermeldungen |
| **Release** | `console=False` | Kein Konsolenfenster — für das fertige Produkt |

### Hinweise

- Der Build nutzt `app_entry.py` als Entry-Point (kein direkter Aufruf von `main.py`, da relative Imports genutzt werden)
- `free-whisper.spec` enthält alle nötigen Konfigurationen für `faster_whisper`, `ctranslate2` und `onnxruntime`
- CUDA-DLLs (`cudnn64_9.dll`) werden bewusst ausgeschlossen — der Frozen-Build nutzt ausschließlich CPU/int8
- Bei Änderungen an den Assets muss neu gebaut werden

---

## Projektstruktur

```
free-whisper/
├── src/free_whisper/     # Quellcode (Package)
│   ├── core/             # Transkriptions-Engine, Audio-Handling
│   └── ui/               # PyQt6-Oberfläche
├── assets/               # Icons, etc.
├── app_entry.py          # PyInstaller Entry-Point
├── free-whisper.spec     # PyInstaller Build-Konfiguration
└── pyproject.toml        # Projekt-Metadaten & Abhängigkeiten
```
