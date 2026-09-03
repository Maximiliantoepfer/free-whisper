# ADR 0003: SQLite is the single source of persistent application data

**Status:** Accepted

Settings, profiles, models, lexicon state, transcripts, corrections and
migration audits live in one versioned SQLite database. Provider tokens never
do; they are referred to by an opaque secret-store reference.
