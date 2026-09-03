from __future__ import annotations

import sqlite3
import threading
from datetime import datetime
from pathlib import Path

from .migrations import run_migrations
from .models import DictEntry, Transcript


class Database:
    def __init__(self, db_path: Path) -> None:
        db_path.parent.mkdir(parents=True, exist_ok=True)
        self._path = db_path
        self._lock = threading.Lock()
        self._conn = sqlite3.connect(str(db_path), check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        run_migrations(self._conn)

    # ------------------------------------------------------------------
    # Transcripts
    # ------------------------------------------------------------------

    def save_transcript(
        self,
        text: str,
        audio_duration_ms: int | None = None,
        model_used: str | None = None,
        language_detected: str | None = None,
    ) -> int:
        with self._lock:
            cur = self._conn.execute(
                """INSERT INTO transcripts (text, audio_duration_ms, model_used, language_detected)
                   VALUES (?, ?, ?, ?)""",
                (text, audio_duration_ms, model_used, language_detected),
            )
            self._conn.commit()
            return cur.lastrowid  # type: ignore[return-value]

    def get_transcripts(self, limit: int = 50, offset: int = 0) -> list[Transcript]:
        with self._lock:
            rows = self._conn.execute(
                """SELECT id, text, created_at, audio_duration_ms, model_used, language_detected
                   FROM transcripts ORDER BY created_at DESC LIMIT ? OFFSET ?""",
                (limit, offset),
            ).fetchall()
        return [self._row_to_transcript(r) for r in rows]

    def search_transcripts(self, query: str) -> list[Transcript]:
        with self._lock:
            rows = self._conn.execute(
                """SELECT t.id, t.text, t.created_at, t.audio_duration_ms,
                          t.model_used, t.language_detected
                   FROM transcripts t
                   JOIN transcripts_fts f ON t.id = f.rowid
                   WHERE transcripts_fts MATCH ?
                   ORDER BY rank""",
                (query,),
            ).fetchall()
        return [self._row_to_transcript(r) for r in rows]

    def delete_transcript(self, transcript_id: int) -> None:
        with self._lock:
            self._conn.execute("DELETE FROM transcripts WHERE id = ?", (transcript_id,))
            self._conn.commit()

    def get_transcript_count(self) -> int:
        with self._lock:
            row = self._conn.execute("SELECT COUNT(*) FROM transcripts").fetchone()
            return row[0] if row else 0

    # ------------------------------------------------------------------
    # Dictionary
    # ------------------------------------------------------------------

    def get_dictionary(self) -> list[DictEntry]:
        with self._lock:
            rows = self._conn.execute(
                "SELECT id, word, category, created_at FROM dictionary ORDER BY category, word"
            ).fetchall()
        return [self._row_to_dict_entry(r) for r in rows]

    def add_word(self, word: str, category: str = "") -> int:
        word = word.strip()
        if not word:
            raise ValueError("Word cannot be empty")
        with self._lock:
            cur = self._conn.execute(
                "INSERT OR IGNORE INTO dictionary (word, category) VALUES (?, ?)",
                (word, category),
            )
            self._conn.commit()
            return cur.lastrowid  # type: ignore[return-value]

    def remove_word(self, word_id: int) -> None:
        with self._lock:
            self._conn.execute("DELETE FROM dictionary WHERE id = ?", (word_id,))
            self._conn.commit()

    def update_word(self, word_id: int, word: str, category: str) -> None:
        with self._lock:
            self._conn.execute(
                "UPDATE dictionary SET word = ?, category = ? WHERE id = ?",
                (word.strip(), category, word_id),
            )
            self._conn.commit()

    def get_hotwords_string(self) -> str:
        """Return all dictionary words joined as a string for faster-whisper hotwords param."""
        with self._lock:
            rows = self._conn.execute("SELECT word FROM dictionary ORDER BY word").fetchall()
        return ", ".join(r[0] for r in rows)

    def get_categories(self) -> list[str]:
        with self._lock:
            rows = self._conn.execute(
                "SELECT DISTINCT category FROM dictionary WHERE category != '' ORDER BY category"
            ).fetchall()
        return [r[0] for r in rows]

    # ------------------------------------------------------------------
    # App settings
    # ------------------------------------------------------------------

    def get_setting(self, key: str, default: str | None = None) -> str | None:
        with self._lock:
            row = self._conn.execute(
                "SELECT value FROM app_settings WHERE key = ?", (key,)
            ).fetchone()
        return row[0] if row else default

    def set_setting(self, key: str, value: str) -> None:
        with self._lock:
            self._conn.execute(
                """INSERT INTO app_settings (key, value, updated_at)
                   VALUES (?, ?, datetime('now'))
                   ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at""",
                (key, value),
            )
            self._conn.commit()

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _row_to_transcript(row: sqlite3.Row) -> Transcript:
        created = datetime.fromisoformat(row["created_at"]) if row["created_at"] else datetime.now()
        return Transcript(
            id=row["id"],
            text=row["text"],
            created_at=created,
            audio_duration_ms=row["audio_duration_ms"],
            model_used=row["model_used"],
            language_detected=row["language_detected"],
        )

    @staticmethod
    def _row_to_dict_entry(row: sqlite3.Row) -> DictEntry:
        created = datetime.fromisoformat(row["created_at"]) if row["created_at"] else datetime.now()
        return DictEntry(
            id=row["id"],
            word=row["word"],
            category=row["category"],
            created_at=created,
        )

    def close(self) -> None:
        with self._lock:
            self._conn.close()
