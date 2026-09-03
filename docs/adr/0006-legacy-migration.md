# ADR 0006: Legacy import is opt-in, backed up and idempotent

**Status:** Accepted

V2 only searches known v0.1 locations after confirmation. It creates a SQLite
backup without modifying the source, previews registry settings, and records
source fingerprints plus imported identifiers to prevent duplicate imports.
