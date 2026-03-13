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

App starten:

```
free-whisper
```

---

## Als .exe kompilieren (Windows)

**Voraussetzungen:** PyInstaller installiert

```
pip install pyinstaller
```

Build ausführen:

```
python -m PyInstaller free-whisper.spec
```

Die fertige `free-whisper.exe` liegt danach unter `dist\free-whisper.exe`.

### Hinweise

- Der Build nutzt `app_entry.py` als Entry-Point (kein direkter Aufruf von `main.py`, da relative Imports genutzt werden)
- `free-whisper.spec` enthält alle nötigen Konfigurationen für `faster_whisper`, `ctranslate2`, `torch` und `onnxruntime`
- Bei Änderungen an den Assets muss neu gebaut werden
- `--onefile` packt alles in eine einzelne EXE (~500 MB+), was den Startvorgang verlangsamt. Für schnelleren Start `onefile=False` in der Spec setzen

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
