# Legacy inventory

This inventory records the v0.1 repository before the v2 reset. It is a
reference for the explicit migration assistant, not a dependency list.

## Tracked Python application

- `src/free_whisper/`: PyQt UI, faster-whisper integration, audio, hotkey,
  injection, SQLite access, settings, and logging.
- `assets/`: legacy icons and QSS styles.
- `app_entry.py`, `build_hook.py`, `rthook_ctranslate2.py`: PyInstaller hooks.
- `free-whisper.spec`, `free_whisper.spec`: competing PyInstaller specs.
- `pyproject.toml`, `requirements.txt`, `requirements-dev.txt`: Python/PyQt6,
  faster-whisper, CTranslate2 and PyInstaller dependencies.
- `scripts/generate_icons.py`, `README.md`, `CLAUDE.md`.

## Legacy user data discovered

The known application database path is `%APPDATA%\\free-whisper\\free_whisper.db`.
The database itself is never moved, deleted, opened writable, or scanned by v2
without a user-confirmed migration. The v0.1 schema has `transcripts`,
`dictionary`, `app_settings`, FTS structures, and `schema_version`.

Legacy QSettings use organisation/application `free-whisper/free-whisper`.
Their registry import is preview-only and requires confirmation.

## Archival operation for M1

Tracked legacy inputs move with `git mv` into `old/python/`, preserving history.
`build/`, `dist/`, and `.venv/` are ignored local build artifacts and may be
deleted only after each resolved path is verified as inside this repository.
No `%APPDATA%` path is in scope for deletion.
