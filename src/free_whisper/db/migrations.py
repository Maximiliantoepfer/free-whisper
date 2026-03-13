from __future__ import annotations

import sqlite3


CURRENT_VERSION = 1

_MIGRATIONS: dict[int, str] = {
    1: """
        CREATE TABLE IF NOT EXISTS transcripts (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            text              TEXT    NOT NULL,
            created_at        TEXT    NOT NULL DEFAULT (datetime('now')),
            audio_duration_ms INTEGER,
            model_used        TEXT,
            language_detected TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_transcripts_created_at
            ON transcripts(created_at DESC);

        CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
            text,
            content='transcripts',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS transcripts_ai
        AFTER INSERT ON transcripts BEGIN
            INSERT INTO transcripts_fts(rowid, text) VALUES (new.id, new.text);
        END;

        CREATE TRIGGER IF NOT EXISTS transcripts_ad
        AFTER DELETE ON transcripts BEGIN
            INSERT INTO transcripts_fts(transcripts_fts, rowid, text)
                VALUES('delete', old.id, old.text);
        END;

        CREATE TABLE IF NOT EXISTS dictionary (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            word       TEXT    NOT NULL UNIQUE COLLATE NOCASE,
            category   TEXT    NOT NULL DEFAULT '',
            created_at TEXT    NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_dictionary_category
            ON dictionary(category);

        CREATE TABLE IF NOT EXISTS app_settings (
            key        TEXT PRIMARY KEY,
            value      TEXT,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
    """,
}


def run_migrations(conn: sqlite3.Connection) -> None:
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)"
    )
    conn.commit()

    row = conn.execute("SELECT version FROM schema_version").fetchone()
    current = row[0] if row else 0

    for version in sorted(_MIGRATIONS):
        if version > current:
            conn.executescript(_MIGRATIONS[version])
            if current == 0:
                conn.execute("INSERT INTO schema_version VALUES (?)", (version,))
            else:
                conn.execute("UPDATE schema_version SET version = ?", (version,))
            conn.commit()
            current = version
