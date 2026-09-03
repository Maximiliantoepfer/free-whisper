#![forbid(unsafe_code)]

//! SQLite is the only persistent store. Secrets never enter this crate: provider
//! profiles hold a credential-manager reference, never a bearer token.

use std::{collections::BTreeMap, path::Path};

use csv::ReaderBuilder;
use free_whisper_domain::{
    AppliedCorrection, InjectionOutcome, LexiconEntryId, LexiconProfileId, LexiconVariantId,
    ModelId, ProcessingDurations, ProviderMetadata, ProviderProfileId, TargetWindowSnapshot,
    TranscriptCorrectionId, TranscriptId, TranscriptSegment,
};
use free_whisper_lexicon::{
    LexiconEntry, LexiconEntryDraft, LexiconProfile, LexiconScope, LexiconValidationError,
    LexiconVariant, LexiconVariantDraft, MatchMode, render_corrections,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

const LATEST_SCHEMA_VERSION: i64 = 3;

#[derive(Debug)]
pub struct Database {
    connection: Connection,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection, OffsetDateTime::now_utc())
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        Self::from_connection(connection, OffsetDateTime::now_utc())
    }

    fn from_connection(
        mut connection: Connection,
        now: OffsetDateTime,
    ) -> Result<Self, StorageError> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;",
        )?;
        apply_migrations(&mut connection, now)?;
        Ok(Self { connection })
    }

    #[must_use]
    pub fn schema_version(&self) -> i64 {
        self.connection
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .expect("schema version exists after successful database initialisation")
    }

    pub fn settings(&mut self) -> SettingsRepository<'_> {
        SettingsRepository {
            connection: &mut self.connection,
        }
    }

    pub fn providers(&mut self) -> ProviderProfileRepository<'_> {
        ProviderProfileRepository {
            connection: &mut self.connection,
        }
    }

    pub fn models(&mut self) -> InstalledModelRepository<'_> {
        InstalledModelRepository {
            connection: &mut self.connection,
        }
    }

    pub fn lexicon(&mut self) -> LexiconRepository<'_> {
        LexiconRepository {
            connection: &mut self.connection,
        }
    }

    pub fn transcripts(&mut self) -> TranscriptRepository<'_> {
        TranscriptRepository {
            connection: &mut self.connection,
        }
    }

    pub fn migration_audit(&mut self) -> MigrationAuditRepository<'_> {
        MigrationAuditRepository {
            connection: &mut self.connection,
        }
    }
}

fn apply_migrations(connection: &mut Connection, now: OffsetDateTime) -> Result<(), StorageError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))?;
    let current: i64 = tx.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    if count != current || current > LATEST_SCHEMA_VERSION {
        return Err(StorageError::InvalidMigrationHistory { count, current });
    }
    if current < 1 {
        tx.execute_batch(MIGRATION_1_SQL)?;
        tx.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            params![1, format_time(now)?],
        )?;
    }
    if current < 2 {
        tx.execute_batch(MIGRATION_2_SQL)?;
        tx.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            params![2, format_time(now)?],
        )?;
    }
    if current < 3 {
        tx.execute_batch(MIGRATION_3_SQL)?;
        tx.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            params![3, format_time(now)?],
        )?;
    }
    tx.commit()?;
    Ok(())
}

const MIGRATION_1_SQL: &str = r#"
CREATE TABLE settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE provider_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    provider_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    endpoint TEXT,
    credential_reference TEXT,
    settings_json TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE installed_models (
    model_id TEXT PRIMARY KEY NOT NULL,
    provider_id TEXT NOT NULL,
    installation_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    backend TEXT NOT NULL,
    installed_at TEXT NOT NULL,
    validated_at TEXT,
    active INTEGER NOT NULL CHECK (active IN (0, 1))
);
CREATE TABLE lexicon_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE lexicon_entries (
    id TEXT PRIMARY KEY NOT NULL,
    canonical_text TEXT NOT NULL,
    canonical_key TEXT NOT NULL,
    language TEXT,
    category TEXT,
    priority INTEGER NOT NULL CHECK (priority BETWEEN 0 AND 100),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'profile')),
    profile_id TEXT REFERENCES lexicon_profiles(id) ON DELETE CASCADE,
    identity_key TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((scope_kind = 'global' AND profile_id IS NULL) OR (scope_kind = 'profile' AND profile_id IS NOT NULL))
);
CREATE TABLE lexicon_variants (
    id TEXT PRIMARY KEY NOT NULL,
    entry_id TEXT NOT NULL REFERENCES lexicon_entries(id) ON DELETE CASCADE,
    variant_text TEXT NOT NULL,
    variant_key TEXT NOT NULL,
    match_mode TEXT NOT NULL CHECK (match_mode IN ('exact_casefold', 'exact_case_sensitive')),
    created_at TEXT NOT NULL,
    UNIQUE (entry_id, variant_key, match_mode)
);
CREATE TABLE transcripts (
    id TEXT PRIMARY KEY NOT NULL,
    created_at TEXT NOT NULL,
    raw_text TEXT NOT NULL,
    final_text TEXT NOT NULL,
    language_requested TEXT,
    language_detected TEXT,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    audio_duration_ms INTEGER NOT NULL CHECK (audio_duration_ms >= 0),
    processing_ms INTEGER NOT NULL CHECK (processing_ms >= 0),
    injection_outcome_json TEXT NOT NULL,
    corrections_json TEXT NOT NULL
);
CREATE TABLE transcript_corrections (
    id TEXT PRIMARY KEY NOT NULL,
    transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    variant_id TEXT NOT NULL,
    original_text TEXT NOT NULL,
    replacement_text TEXT NOT NULL,
    start_char INTEGER NOT NULL CHECK (start_char >= 0),
    end_char INTEGER NOT NULL CHECK (end_char >= start_char),
    applied_at TEXT NOT NULL,
    reverted_at TEXT
);
CREATE TABLE migration_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_kind TEXT NOT NULL,
    source_fingerprint TEXT NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    report_json TEXT NOT NULL,
    UNIQUE (source_kind, source_fingerprint)
);
CREATE INDEX idx_transcripts_created_at ON transcripts(created_at DESC);
CREATE INDEX idx_transcript_corrections_transcript ON transcript_corrections(transcript_id);
CREATE INDEX idx_lexicon_entries_scope ON lexicon_entries(scope_kind, profile_id, enabled, priority DESC);
"#;

const MIGRATION_2_SQL: &str = r#"
ALTER TABLE transcripts ADD COLUMN language_confidence_milli INTEGER;
ALTER TABLE transcripts ADD COLUMN provider_metadata_json TEXT NOT NULL DEFAULT 'null';
ALTER TABLE transcripts ADD COLUMN segments_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE transcripts ADD COLUMN warnings_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE transcripts ADD COLUMN durations_json TEXT NOT NULL DEFAULT '{"queue_ms":0,"model_load_ms":0,"inference_ms":0,"correction_ms":0}';
"#;

const MIGRATION_3_SQL: &str = r#"
ALTER TABLE transcripts ADD COLUMN target_window_json TEXT NOT NULL DEFAULT 'null';
"#;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderProfileRecord {
    pub id: ProviderProfileId,
    pub provider_id: String,
    pub display_name: String,
    pub endpoint: Option<String>,
    /// Identifier for Windows Credential Manager; deliberately never a secret.
    pub credential_reference: Option<String>,
    pub settings: ProviderProfileSettings,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderProfileSettings {
    pub connect_timeout_ms: u32,
    pub request_timeout_ms: u32,
    pub developer_allow_http_loopback: bool,
    /// The ready model selected on the user's own worker. This is metadata,
    /// not a local file path and never a bearer token.
    #[serde(default)]
    pub remote_model_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstalledModelRecord {
    pub model_id: ModelId,
    pub provider_id: String,
    pub installation_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub backend: String,
    pub installed_at: OffsetDateTime,
    pub validated_at: Option<OffsetDateTime>,
    pub active: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredTranscript {
    pub id: TranscriptId,
    pub created_at: OffsetDateTime,
    pub raw_text: String,
    pub final_text: String,
    pub language_requested: Option<String>,
    pub language_detected: Option<String>,
    pub language_confidence_milli: Option<u16>,
    pub provider_id: String,
    pub model_id: ModelId,
    pub audio_duration_ms: u64,
    pub processing_ms: u64,
    pub injection_outcome: InjectionOutcome,
    /// Present only for an opt-in original-window paste request. Window titles
    /// may contain sensitive context, so copy-only records keep this `None`.
    pub target_window: Option<TargetWindowSnapshot>,
    pub provider_metadata: Option<ProviderMetadata>,
    pub segments: Vec<TranscriptSegment>,
    pub warnings: Vec<String>,
    pub durations: ProcessingDurations,
    pub corrections: Vec<StoredCorrection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredCorrection {
    #[serde(flatten)]
    pub correction: AppliedCorrection,
    pub reverted_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MigrationAuditRecord {
    pub source_kind: String,
    pub source_fingerprint: String,
    pub started_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
    pub report: Value,
}

pub struct SettingsRepository<'connection> {
    connection: &'connection mut Connection,
}

impl SettingsRepository<'_> {
    pub fn set(
        &mut self,
        key: &str,
        value: &Value,
        now: OffsetDateTime,
    ) -> Result<(), StorageError> {
        validate_key(key)?;
        if contains_sensitive_key(value) {
            return Err(StorageError::InvalidRecord(
                "settings must not contain token, secret, password or credential fields",
            ));
        }
        self.connection.execute(
            "INSERT INTO settings (key, value_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![key, serde_json::to_string(value)?, format_time(now)?],
        )?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Option<Value>, StorageError> {
        validate_key(key)?;
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).map_err(StorageError::from))
            .transpose()
    }
}

pub struct ProviderProfileRepository<'connection> {
    connection: &'connection mut Connection,
}

impl ProviderProfileRepository<'_> {
    pub fn save(&mut self, profile: &ProviderProfileRecord) -> Result<(), StorageError> {
        if profile.provider_id.trim().is_empty() || profile.display_name.trim().is_empty() {
            return Err(StorageError::InvalidRecord(
                "provider id and display name are required",
            ));
        }
        self.connection.execute(
            "INSERT INTO provider_profiles
                (id, provider_id, display_name, endpoint, credential_reference, settings_json, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                provider_id = excluded.provider_id, display_name = excluded.display_name,
                endpoint = excluded.endpoint, credential_reference = excluded.credential_reference,
                settings_json = excluded.settings_json, enabled = excluded.enabled,
                updated_at = excluded.updated_at",
            params![
                profile.id.to_string(), profile.provider_id, profile.display_name, profile.endpoint,
                profile.credential_reference, serde_json::to_string(&profile.settings)?, bool_to_int(profile.enabled),
                format_time(profile.created_at)?, format_time(profile.updated_at)?
            ],
        )?;
        Ok(())
    }

    pub fn get(
        &self,
        id: ProviderProfileId,
    ) -> Result<Option<ProviderProfileRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT id, provider_id, display_name, endpoint, credential_reference, settings_json, enabled, created_at, updated_at
                 FROM provider_profiles WHERE id = ?1",
                [id.to_string()],
                provider_profile_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn list(&self) -> Result<Vec<ProviderProfileRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, provider_id, display_name, endpoint, credential_reference, settings_json, enabled, created_at, updated_at
             FROM provider_profiles ORDER BY display_name COLLATE NOCASE ASC, id ASC",
        )?;
        statement
            .query_map([], provider_profile_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn delete(&mut self, id: ProviderProfileId) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "DELETE FROM provider_profiles WHERE id = ?1",
            [id.to_string()],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound("provider profile"));
        }
        Ok(())
    }
}

pub struct InstalledModelRepository<'connection> {
    connection: &'connection mut Connection,
}

impl InstalledModelRepository<'_> {
    pub fn save(&mut self, model: &InstalledModelRecord) -> Result<(), StorageError> {
        if model.sha256.len() != 64 || !model.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidRecord(
                "model SHA-256 must contain 64 hexadecimal characters",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if model.active {
            tx.execute(
                "UPDATE installed_models SET active = 0
                 WHERE provider_id = ?1 AND backend = ?2",
                params![model.provider_id, model.backend],
            )?;
        }
        tx.execute(
            "INSERT INTO installed_models
              (model_id, provider_id, installation_path, sha256, size_bytes, backend, installed_at, validated_at, active)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(model_id) DO UPDATE SET provider_id = excluded.provider_id,
              installation_path = excluded.installation_path, sha256 = excluded.sha256,
              size_bytes = excluded.size_bytes, backend = excluded.backend,
              installed_at = excluded.installed_at, validated_at = excluded.validated_at, active = excluded.active",
            params![
                model.model_id.as_str(), model.provider_id, model.installation_path, model.sha256,
                u64_to_i64(model.size_bytes)?, model.backend, format_time(model.installed_at)?,
                model.validated_at.map(format_time).transpose()?, bool_to_int(model.active)
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get(&self, id: &ModelId) -> Result<Option<InstalledModelRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT model_id, provider_id, installation_path, sha256, size_bytes, backend, installed_at, validated_at, active
                 FROM installed_models WHERE model_id = ?1",
                [id.as_str()],
                installed_model_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn delete(&mut self, id: &ModelId) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "DELETE FROM installed_models WHERE model_id = ?1",
            [id.as_str()],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound("installed model"));
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<InstalledModelRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT model_id, provider_id, installation_path, sha256, size_bytes, backend, installed_at, validated_at, active
             FROM installed_models ORDER BY active DESC, model_id ASC",
        )?;
        statement
            .query_map([], installed_model_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    /// Atomically selects one validated local CPU model. A failed validation
    /// leaves the previous selection untouched.
    pub fn activate_local_cpu(&mut self, id: &ModelId) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM installed_models
                 WHERE model_id = ?1 AND provider_id = 'local-whisper-cpp'
                   AND backend = 'cpu' AND validated_at IS NOT NULL",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        if candidate.is_none() {
            return Err(StorageError::NotFound("validated local CPU model"));
        }
        tx.execute(
            "UPDATE installed_models SET active = 0
             WHERE provider_id = 'local-whisper-cpp' AND backend = 'cpu'",
            [],
        )?;
        tx.execute(
            "UPDATE installed_models SET active = 1 WHERE model_id = ?1",
            [id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn active_local_cpu(&self) -> Result<Option<InstalledModelRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT model_id, provider_id, installation_path, sha256, size_bytes, backend,
                        installed_at, validated_at, active
                 FROM installed_models
                 WHERE provider_id = 'local-whisper-cpp' AND backend = 'cpu' AND active = 1",
                [],
                installed_model_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }
}

pub struct LexiconRepository<'connection> {
    connection: &'connection mut Connection,
}

impl LexiconRepository<'_> {
    pub fn save_profile(&mut self, profile: &LexiconProfile) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO lexicon_profiles (id, name, normalized_name, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, normalized_name = excluded.normalized_name,
                 enabled = excluded.enabled, updated_at = excluded.updated_at",
            params![
                profile.id.to_string(), profile.name, profile.normalized_name, bool_to_int(profile.enabled),
                format_time(profile.created_at)?, format_time(profile.updated_at)?
            ],
        )?;
        Ok(())
    }

    pub fn profiles(&self) -> Result<Vec<LexiconProfile>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, normalized_name, enabled, created_at, updated_at
             FROM lexicon_profiles ORDER BY normalized_name ASC, id ASC",
        )?;
        statement
            .query_map([], lexicon_profile_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn delete_profile(&mut self, id: LexiconProfileId) -> Result<(), StorageError> {
        let affected = self.connection.execute(
            "DELETE FROM lexicon_profiles WHERE id = ?1",
            [id.to_string()],
        )?;
        if affected == 1 {
            Ok(())
        } else {
            Err(StorageError::NotFound("lexicon profile"))
        }
    }

    pub fn save_entry(
        &mut self,
        entry: &LexiconEntry,
        variants: &[LexiconVariant],
    ) -> Result<(), StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_entry_and_variants(&tx, entry, variants)?;
        tx.commit()?;
        Ok(())
    }

    pub fn entries(&self) -> Result<Vec<LexiconEntry>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, canonical_text, canonical_key, language, category, priority, enabled,
                    scope_kind, profile_id, created_at, updated_at
             FROM lexicon_entries ORDER BY priority DESC, canonical_key ASC, id ASC",
        )?;
        statement
            .query_map([], lexicon_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn variants_for(
        &self,
        entry_id: LexiconEntryId,
    ) -> Result<Vec<LexiconVariant>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, entry_id, variant_text, variant_key, match_mode, created_at
             FROM lexicon_variants WHERE entry_id = ?1 ORDER BY variant_key ASC, id ASC",
        )?;
        statement
            .query_map([entry_id.to_string()], lexicon_variant_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn delete_entry(&mut self, id: LexiconEntryId) -> Result<(), StorageError> {
        let affected = self.connection.execute(
            "DELETE FROM lexicon_entries WHERE id = ?1",
            [id.to_string()],
        )?;
        if affected == 1 {
            Ok(())
        } else {
            Err(StorageError::NotFound("lexicon entry"))
        }
    }

    /// Validates every document row before it opens an SQL transaction.
    pub fn import(
        &mut self,
        drafts: &[LexiconEntryDraft],
        now: OffsetDateTime,
    ) -> Result<LexiconImportReport, StorageError> {
        let mut validated = Vec::with_capacity(drafts.len());
        let mut errors = Vec::new();
        for (index, draft) in drafts.iter().enumerate() {
            match draft.validate(now) {
                Ok(value) => validated.push(value),
                Err(error) => errors.push(ImportValidationIssue {
                    item_index: index,
                    message: error.to_string(),
                }),
            }
        }
        if !errors.is_empty() {
            return Err(StorageError::ImportValidation(errors));
        }

        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = LexiconImportReport::default();
        let mut seen_identity = BTreeMap::<String, usize>::new();
        for (index, draft) in validated.iter().enumerate() {
            let identity_key = draft.entry.identity_key();
            if seen_identity.insert(identity_key.clone(), index).is_some()
                || entry_id_by_identity(&tx, &identity_key)?.is_some()
            {
                report.duplicate_entries += 1;
                report.skipped += 1;
                continue;
            }
            let mut unique_variants = Vec::with_capacity(draft.variants.len());
            let mut seen_variant_keys = BTreeMap::new();
            for variant in &draft.variants {
                let key = format!("{}|{:?}", variant.variant_key, variant.match_mode);
                if seen_variant_keys.insert(key, variant.id).is_some() {
                    report.duplicate_variants += 1;
                    continue;
                }
                unique_variants.push(variant.clone());
            }
            insert_entry_and_variants(&tx, &draft.entry, &unique_variants)?;
            report.added_entries += 1;
            report.added_variants += unique_variants.len();
        }
        tx.commit()?;
        Ok(report)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LexiconImportReport {
    pub added_entries: usize,
    pub added_variants: usize,
    pub duplicate_entries: usize,
    pub duplicate_variants: usize,
    pub skipped: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportValidationIssue {
    pub item_index: usize,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CsvLexiconRow {
    pub canonical_text: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    pub priority: u8,
    #[serde(default = "default_csv_enabled")]
    pub enabled: bool,
    pub scope: String,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub variant_text: Option<String>,
    #[serde(default)]
    pub match_mode: Option<String>,
}

const fn default_csv_enabled() -> bool {
    true
}

pub fn parse_lexicon_csv(csv: &str) -> Result<Vec<LexiconEntryDraft>, StorageError> {
    let mut reader = ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(csv.as_bytes());
    let mut grouped = BTreeMap::<String, LexiconEntryDraft>::new();
    for (row_index, row) in reader.deserialize::<CsvLexiconRow>().enumerate() {
        let row = row.map_err(|error| StorageError::ImportParse {
            row: row_index + 2,
            message: error.to_string(),
        })?;
        let scope = csv_scope(&row.scope, row.profile_id.as_deref()).map_err(|message| {
            StorageError::ImportParse {
                row: row_index + 2,
                message,
            }
        })?;
        let identity = format!(
            "{}|{}|{}|{:?}",
            row.canonical_text,
            row.language.as_deref().unwrap_or_default(),
            row.category.as_deref().unwrap_or_default(),
            scope
        );
        let entry = grouped
            .entry(identity)
            .or_insert_with(|| LexiconEntryDraft {
                canonical_text: row.canonical_text.clone(),
                language: row.language.clone(),
                category: row.category.clone(),
                priority: row.priority,
                enabled: row.enabled,
                scope: scope.clone(),
                variants: Vec::new(),
            });
        if entry.priority != row.priority || entry.enabled != row.enabled {
            return Err(StorageError::ImportParse {
                row: row_index + 2,
                message:
                    "rows grouped into one entry must have identical priority and enabled values"
                        .to_owned(),
            });
        }
        match (row.variant_text, row.match_mode) {
            (None, None) => {}
            (Some(variant_text), Some(match_mode)) => entry.variants.push(LexiconVariantDraft {
                variant_text,
                match_mode: parse_match_mode(&match_mode).map_err(|message| {
                    StorageError::ImportParse {
                        row: row_index + 2,
                        message,
                    }
                })?,
            }),
            _ => {
                return Err(StorageError::ImportParse {
                    row: row_index + 2,
                    message: "variant_text and match_mode must be provided together".to_owned(),
                });
            }
        }
    }
    Ok(grouped.into_values().collect())
}

pub fn parse_lexicon_json(json: &str) -> Result<Vec<LexiconEntryDraft>, StorageError> {
    serde_json::from_str(json).map_err(StorageError::from)
}

pub struct TranscriptRepository<'connection> {
    connection: &'connection mut Connection,
}

impl TranscriptRepository<'_> {
    pub fn save(&mut self, transcript: &StoredTranscript) -> Result<(), StorageError> {
        let active_corrections = transcript
            .corrections
            .iter()
            .filter(|stored| stored.reverted_at.is_none())
            .map(|stored| stored.correction.clone())
            .collect::<Vec<_>>();
        let expected_final = render_corrections(&transcript.raw_text, &active_corrections)
            .map_err(|error| StorageError::InvalidCorrection(error.to_string()))?;
        if expected_final != transcript.final_text {
            return Err(StorageError::InvalidCorrection(
                "final text does not match immutable raw text and stored corrections".to_owned(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO transcripts
             (id, created_at, raw_text, final_text, language_requested, language_detected, provider_id, model_id,
              audio_duration_ms, processing_ms, injection_outcome_json, corrections_json,
              language_confidence_milli, provider_metadata_json, segments_json, warnings_json, durations_json,
              target_window_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                transcript.id.to_string(), format_time(transcript.created_at)?, transcript.raw_text,
                transcript.final_text, transcript.language_requested, transcript.language_detected,
                transcript.provider_id, transcript.model_id.as_str(), u64_to_i64(transcript.audio_duration_ms)?,
                u64_to_i64(transcript.processing_ms)?, serde_json::to_string(&transcript.injection_outcome)?,
                serde_json::to_string(&transcript.corrections)?, transcript.language_confidence_milli,
                serde_json::to_string(&transcript.provider_metadata)?, serde_json::to_string(&transcript.segments)?,
                serde_json::to_string(&transcript.warnings)?, serde_json::to_string(&transcript.durations)?,
                serde_json::to_string(&transcript.target_window)?
            ],
        )?;
        for stored in &transcript.corrections {
            insert_correction(&tx, transcript.id, stored)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get(&self, id: TranscriptId) -> Result<Option<StoredTranscript>, StorageError> {
        let mut transcript = self
            .connection
            .query_row(
                "SELECT id, created_at, raw_text, final_text, language_requested, language_detected, provider_id,
                 model_id, audio_duration_ms, processing_ms, injection_outcome_json,
                 language_confidence_milli, provider_metadata_json, segments_json, warnings_json, durations_json,
                 target_window_json
                 FROM transcripts WHERE id = ?1",
                [id.to_string()],
                transcript_from_row,
            )
            .optional()?;
        if let Some(value) = &mut transcript {
            value.corrections = corrections_for(self.connection, id, true)?;
        }
        Ok(transcript)
    }

    pub fn list_recent(&self, limit: u16) -> Result<Vec<StoredTranscript>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, created_at, raw_text, final_text, language_requested, language_detected, provider_id,
                    model_id, audio_duration_ms, processing_ms, injection_outcome_json,
                    language_confidence_milli, provider_metadata_json, segments_json, warnings_json, durations_json,
                    target_window_json
             FROM transcripts ORDER BY created_at DESC, id DESC LIMIT ?1",
        )?;
        let mut transcripts = statement
            .query_map([i64::from(limit)], transcript_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        for transcript in &mut transcripts {
            transcript.corrections = corrections_for(self.connection, transcript.id, true)?;
        }
        Ok(transcripts)
    }

    /// Searches literal user text with bound parameters. It deliberately does not
    /// expose SQLite FTS query syntax, so punctuation can never turn into a hidden
    /// query parser error or an unintended wildcard.
    pub fn search(&self, query: &str, limit: u16) -> Result<Vec<StoredTranscript>, StorageError> {
        let query = query.trim();
        if query.is_empty() || query.len() > 512 || query.chars().any(char::is_control) {
            return Err(StorageError::InvalidSearch);
        }
        let pattern = format!("%{}%", escape_like(query));
        let mut statement = self.connection.prepare(
            "SELECT id, created_at, raw_text, final_text, language_requested, language_detected, provider_id,
                    model_id, audio_duration_ms, processing_ms, injection_outcome_json,
                    language_confidence_milli, provider_metadata_json, segments_json, warnings_json, durations_json,
                    target_window_json
             FROM transcripts
             WHERE raw_text LIKE ?1 ESCAPE '\\' OR final_text LIKE ?1 ESCAPE '\\'
             ORDER BY created_at DESC, id DESC LIMIT ?2",
        )?;
        let mut transcripts = statement
            .query_map(params![pattern, i64::from(limit)], transcript_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        for transcript in &mut transcripts {
            transcript.corrections = corrections_for(self.connection, transcript.id, true)?;
        }
        Ok(transcripts)
    }

    pub fn revert_correction(
        &mut self,
        transcript_id: TranscriptId,
        correction_id: TranscriptCorrectionId,
        now: OffsetDateTime,
    ) -> Result<String, StorageError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raw_text: String = tx
            .query_row(
                "SELECT raw_text FROM transcripts WHERE id = ?1",
                [transcript_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StorageError::NotFound("transcript"))?;
        let affected = tx.execute(
            "UPDATE transcript_corrections SET reverted_at = ?1
             WHERE id = ?2 AND transcript_id = ?3 AND reverted_at IS NULL",
            params![
                format_time(now)?,
                correction_id.to_string(),
                transcript_id.to_string()
            ],
        )?;
        if affected != 1 {
            return Err(StorageError::NotFound("active transcript correction"));
        }
        let remaining = corrections_for(&tx, transcript_id, false)?
            .into_iter()
            .map(|stored| stored.correction)
            .collect::<Vec<_>>();
        let all_corrections = corrections_for(&tx, transcript_id, true)?;
        let final_text = render_corrections(&raw_text, &remaining)
            .map_err(|error| StorageError::InvalidCorrection(error.to_string()))?;
        tx.execute(
            "UPDATE transcripts SET final_text = ?1, corrections_json = ?2 WHERE id = ?3",
            params![
                final_text,
                serde_json::to_string(&all_corrections)?,
                transcript_id.to_string()
            ],
        )?;
        tx.commit()?;
        Ok(final_text)
    }

    pub fn update_injection_outcome(
        &mut self,
        transcript_id: TranscriptId,
        outcome: &InjectionOutcome,
    ) -> Result<(), StorageError> {
        let affected = self.connection.execute(
            "UPDATE transcripts SET injection_outcome_json = ?1 WHERE id = ?2",
            params![serde_json::to_string(outcome)?, transcript_id.to_string()],
        )?;
        if affected == 1 {
            Ok(())
        } else {
            Err(StorageError::NotFound("transcript"))
        }
    }

    /// Appends a visible operational warning without rewriting immutable text or
    /// correction history. This is used for clipboard restore races.
    pub fn append_warning(
        &mut self,
        transcript_id: TranscriptId,
        warning: &str,
    ) -> Result<(), StorageError> {
        if warning.trim().is_empty() {
            return Err(StorageError::InvalidRecord("empty transcript warning"));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: String = transaction
            .query_row(
                "SELECT warnings_json FROM transcripts WHERE id = ?1",
                [transcript_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StorageError::NotFound("transcript"))?;
        let mut warnings: Vec<String> = serde_json::from_str(&existing)?;
        warnings.push(warning.to_owned());
        transaction.execute(
            "UPDATE transcripts SET warnings_json = ?1 WHERE id = ?2",
            params![serde_json::to_string(&warnings)?, transcript_id.to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

pub struct MigrationAuditRepository<'connection> {
    connection: &'connection mut Connection,
}

impl MigrationAuditRepository<'_> {
    pub fn record(&mut self, record: &MigrationAuditRecord) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO migration_audit (source_kind, source_fingerprint, started_at, completed_at, report_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.source_kind, record.source_fingerprint, format_time(record.started_at)?,
                record.completed_at.map(format_time).transpose()?, serde_json::to_string(&record.report)?
            ],
        )?;
        Ok(())
    }

    pub fn already_imported(
        &self,
        source_kind: &str,
        fingerprint: &str,
    ) -> Result<bool, StorageError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM migration_audit WHERE source_kind = ?1 AND source_fingerprint = ?2",
            [source_kind, fingerprint],
            |row| row.get(0),
        )?;
        Ok(count != 0)
    }
}

fn insert_entry_and_variants(
    tx: &Transaction<'_>,
    entry: &LexiconEntry,
    variants: &[LexiconVariant],
) -> Result<(), StorageError> {
    let (scope_kind, profile_id) = scope_to_columns(&entry.scope);
    tx.execute(
        "INSERT INTO lexicon_entries
         (id, canonical_text, canonical_key, language, category, priority, enabled, scope_kind, profile_id,
          identity_key, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            entry.id.to_string(), entry.canonical_text, entry.canonical_key, entry.language, entry.category,
            i64::from(entry.priority), bool_to_int(entry.enabled), scope_kind, profile_id, entry.identity_key(),
            format_time(entry.created_at)?, format_time(entry.updated_at)?
        ],
    )?;
    for variant in variants {
        if variant.entry_id != entry.id {
            return Err(StorageError::InvalidRecord(
                "variant belongs to a different lexicon entry",
            ));
        }
        tx.execute(
            "INSERT INTO lexicon_variants (id, entry_id, variant_text, variant_key, match_mode, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                variant.id.to_string(), variant.entry_id.to_string(), variant.variant_text, variant.variant_key,
                match_mode_to_str(variant.match_mode), format_time(variant.created_at)?
            ],
        )?;
    }
    Ok(())
}

fn entry_id_by_identity(
    tx: &Transaction<'_>,
    identity_key: &str,
) -> Result<Option<String>, StorageError> {
    tx.query_row(
        "SELECT id FROM lexicon_entries WHERE identity_key = ?1",
        [identity_key],
        |row| row.get(0),
    )
    .optional()
    .map_err(StorageError::from)
}

fn insert_correction(
    tx: &Transaction<'_>,
    transcript_id: TranscriptId,
    stored: &StoredCorrection,
) -> Result<(), StorageError> {
    let correction = &stored.correction;
    tx.execute(
        "INSERT INTO transcript_corrections
         (id, transcript_id, variant_id, original_text, replacement_text, start_char, end_char, applied_at, reverted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            correction.id.to_string(), transcript_id.to_string(), correction.variant_id.to_string(),
            correction.original, correction.replacement, i64::from(correction.start_char),
            i64::from(correction.end_char), format_time(correction.applied_at)?,
            stored.reverted_at.map(format_time).transpose()?
        ],
    )?;
    Ok(())
}

fn corrections_for(
    connection: &Connection,
    transcript_id: TranscriptId,
    include_reverted: bool,
) -> Result<Vec<StoredCorrection>, StorageError> {
    let query = if include_reverted {
        "SELECT id, variant_id, original_text, replacement_text, start_char, end_char, applied_at, reverted_at
         FROM transcript_corrections WHERE transcript_id = ?1 ORDER BY start_char, end_char, id"
    } else {
        "SELECT id, variant_id, original_text, replacement_text, start_char, end_char, applied_at, reverted_at
         FROM transcript_corrections WHERE transcript_id = ?1 AND reverted_at IS NULL ORDER BY start_char, end_char, id"
    };
    let mut statement = connection.prepare(query)?;
    statement
        .query_map([transcript_id.to_string()], stored_correction_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn provider_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderProfileRecord> {
    parse_provider_profile_row(row).map_err(to_sql_error)
}

fn parse_provider_profile_row(
    row: &rusqlite::Row<'_>,
) -> Result<ProviderProfileRecord, StorageError> {
    Ok(ProviderProfileRecord {
        id: provider_profile_id(&row.get::<_, String>(0)?)?,
        provider_id: row.get(1)?,
        display_name: row.get(2)?,
        endpoint: row.get(3)?,
        credential_reference: row.get(4)?,
        settings: serde_json::from_str(&row.get::<_, String>(5)?)?,
        enabled: int_to_bool(row.get(6)?)?,
        created_at: parse_time(&row.get::<_, String>(7)?)?,
        updated_at: parse_time(&row.get::<_, String>(8)?)?,
    })
}

fn installed_model_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstalledModelRecord> {
    parse_installed_model_row(row).map_err(to_sql_error)
}

fn parse_installed_model_row(
    row: &rusqlite::Row<'_>,
) -> Result<InstalledModelRecord, StorageError> {
    let model_id: String = row.get(0)?;
    Ok(InstalledModelRecord {
        model_id: ModelId::parse(model_id)
            .map_err(|error| StorageError::InvalidRecordDetail(error.to_string()))?,
        provider_id: row.get(1)?,
        installation_path: row.get(2)?,
        sha256: row.get(3)?,
        size_bytes: i64_to_u64(row.get(4)?)?,
        backend: row.get(5)?,
        installed_at: parse_time(&row.get::<_, String>(6)?)?,
        validated_at: row
            .get::<_, Option<String>>(7)?
            .map(|value| parse_time(&value))
            .transpose()?,
        active: int_to_bool(row.get(8)?)?,
    })
}

fn lexicon_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LexiconEntry> {
    parse_lexicon_entry_row(row).map_err(to_sql_error)
}

fn lexicon_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LexiconProfile> {
    parse_lexicon_profile_row(row).map_err(to_sql_error)
}

fn parse_lexicon_profile_row(row: &rusqlite::Row<'_>) -> Result<LexiconProfile, StorageError> {
    Ok(LexiconProfile {
        id: lexicon_profile_id(&row.get::<_, String>(0)?)?,
        name: row.get(1)?,
        normalized_name: row.get(2)?,
        enabled: int_to_bool(row.get(3)?)?,
        created_at: parse_time(&row.get::<_, String>(4)?)?,
        updated_at: parse_time(&row.get::<_, String>(5)?)?,
    })
}

fn parse_lexicon_entry_row(row: &rusqlite::Row<'_>) -> Result<LexiconEntry, StorageError> {
    let scope_kind: String = row.get(7)?;
    let scope = match scope_kind.as_str() {
        "global" => LexiconScope::Global,
        "profile" => LexiconScope::Profile(lexicon_profile_id(
            &row.get::<_, Option<String>>(8)?
                .ok_or(StorageError::InvalidRecord(
                    "profile scope without profile id",
                ))?,
        )?),
        _ => return Err(StorageError::InvalidRecord("unknown lexicon scope")),
    };
    Ok(LexiconEntry {
        id: lexicon_entry_id(&row.get::<_, String>(0)?)?,
        canonical_text: row.get(1)?,
        canonical_key: row.get(2)?,
        language: row.get(3)?,
        category: row.get(4)?,
        priority: u8::try_from(row.get::<_, i64>(5)?)
            .map_err(|_| StorageError::InvalidRecord("invalid priority"))?,
        enabled: int_to_bool(row.get(6)?)?,
        scope,
        created_at: parse_time(&row.get::<_, String>(9)?)?,
        updated_at: parse_time(&row.get::<_, String>(10)?)?,
    })
}

fn lexicon_variant_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LexiconVariant> {
    parse_lexicon_variant_row(row).map_err(to_sql_error)
}

fn parse_lexicon_variant_row(row: &rusqlite::Row<'_>) -> Result<LexiconVariant, StorageError> {
    let match_mode: String = row.get(4)?;
    Ok(LexiconVariant {
        id: lexicon_variant_id(&row.get::<_, String>(0)?)?,
        entry_id: lexicon_entry_id(&row.get::<_, String>(1)?)?,
        variant_text: row.get(2)?,
        variant_key: row.get(3)?,
        match_mode: parse_match_mode(&match_mode)
            .map_err(|_| StorageError::InvalidRecord("invalid match mode"))?,
        created_at: parse_time(&row.get::<_, String>(5)?)?,
    })
}

fn transcript_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredTranscript> {
    parse_transcript_row(row).map_err(to_sql_error)
}

fn parse_transcript_row(row: &rusqlite::Row<'_>) -> Result<StoredTranscript, StorageError> {
    let model_id: String = row.get(7)?;
    Ok(StoredTranscript {
        id: transcript_id(&row.get::<_, String>(0)?)?,
        created_at: parse_time(&row.get::<_, String>(1)?)?,
        raw_text: row.get(2)?,
        final_text: row.get(3)?,
        language_requested: row.get(4)?,
        language_detected: row.get(5)?,
        provider_id: row.get(6)?,
        model_id: ModelId::parse(model_id)
            .map_err(|error| StorageError::InvalidRecordDetail(error.to_string()))?,
        audio_duration_ms: i64_to_u64(row.get(8)?)?,
        processing_ms: i64_to_u64(row.get(9)?)?,
        injection_outcome: serde_json::from_str(&row.get::<_, String>(10)?)?,
        language_confidence_milli: row
            .get::<_, Option<i64>>(11)?
            .map(|value| {
                u16::try_from(value)
                    .map_err(|_| StorageError::InvalidRecord("invalid language confidence"))
            })
            .transpose()?,
        provider_metadata: serde_json::from_str(&row.get::<_, String>(12)?)?,
        segments: serde_json::from_str(&row.get::<_, String>(13)?)?,
        warnings: serde_json::from_str(&row.get::<_, String>(14)?)?,
        durations: serde_json::from_str(&row.get::<_, String>(15)?)?,
        target_window: serde_json::from_str(&row.get::<_, String>(16)?)?,
        corrections: Vec::new(),
    })
}

fn stored_correction_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCorrection> {
    parse_stored_correction_row(row).map_err(to_sql_error)
}

fn parse_stored_correction_row(row: &rusqlite::Row<'_>) -> Result<StoredCorrection, StorageError> {
    Ok(StoredCorrection {
        correction: AppliedCorrection {
            id: transcript_correction_id(&row.get::<_, String>(0)?)?,
            variant_id: lexicon_variant_id(&row.get::<_, String>(1)?)?,
            original: row.get(2)?,
            replacement: row.get(3)?,
            start_char: u32::try_from(row.get::<_, i64>(4)?)
                .map_err(|_| StorageError::InvalidRecord("invalid correction start"))?,
            end_char: u32::try_from(row.get::<_, i64>(5)?)
                .map_err(|_| StorageError::InvalidRecord("invalid correction end"))?,
            applied_at: parse_time(&row.get::<_, String>(6)?)?,
        },
        reverted_at: row
            .get::<_, Option<String>>(7)?
            .map(|value| parse_time(&value))
            .transpose()?,
    })
}

fn csv_scope(scope: &str, profile_id: Option<&str>) -> Result<LexiconScope, String> {
    match scope {
        "global" => {
            if profile_id.is_some() {
                Err("global scope must not contain profile_id".to_owned())
            } else {
                Ok(LexiconScope::Global)
            }
        }
        "profile" => profile_id
            .ok_or_else(|| "profile scope requires profile_id".to_owned())
            .and_then(|id| {
                lexicon_profile_id(id)
                    .map(LexiconScope::Profile)
                    .map_err(|error| error.to_string())
            }),
        _ => Err("scope must be global or profile".to_owned()),
    }
}

fn scope_to_columns(scope: &LexiconScope) -> (&'static str, Option<String>) {
    match scope {
        LexiconScope::Global => ("global", None),
        LexiconScope::Profile(id) => ("profile", Some(id.to_string())),
    }
}

fn parse_match_mode(value: &str) -> Result<MatchMode, String> {
    match value {
        "exact_casefold" => Ok(MatchMode::ExactCasefold),
        "exact_case_sensitive" => Ok(MatchMode::ExactCaseSensitive),
        _ => Err("match_mode must be exact_casefold or exact_case_sensitive".to_owned()),
    }
}

const fn match_mode_to_str(mode: MatchMode) -> &'static str {
    match mode {
        MatchMode::ExactCasefold => "exact_casefold",
        MatchMode::ExactCaseSensitive => "exact_case_sensitive",
    }
}

fn validate_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty()
        || key.len() > 128
        || key.chars().any(char::is_control)
        || is_sensitive_name(key)
    {
        return Err(StorageError::InvalidRecord("invalid settings key"));
    }
    Ok(())
}

fn contains_sensitive_key(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_sensitive_key),
        Value::Object(values) => values
            .iter()
            .any(|(key, nested)| is_sensitive_name(key) || contains_sensitive_key(nested)),
        _ => false,
    }
}

fn is_sensitive_name(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    ["token", "secret", "password", "credential", "authorization"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

const fn bool_to_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn int_to_bool(value: i64) -> Result<bool, StorageError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StorageError::InvalidRecord("invalid persisted boolean")),
    }
}

fn u64_to_i64(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::InvalidRecord("integer exceeds SQLite range"))
}

fn i64_to_u64(value: i64) -> Result<u64, StorageError> {
    u64::try_from(value)
        .map_err(|_| StorageError::InvalidRecord("negative persisted unsigned integer"))
}

fn format_time(value: OffsetDateTime) -> Result<String, StorageError> {
    value
        .format(&Rfc3339)
        .map_err(|error| StorageError::Time(error.to_string()))
}

fn parse_time(value: &str) -> Result<OffsetDateTime, StorageError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| StorageError::Time(error.to_string()))
}

fn parse_uuid(value: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidRecord("invalid persisted UUID"))
}

fn provider_profile_id(value: &str) -> Result<ProviderProfileId, StorageError> {
    Ok(ProviderProfileId::from_uuid(parse_uuid(value)?))
}

fn lexicon_profile_id(value: &str) -> Result<LexiconProfileId, StorageError> {
    Ok(LexiconProfileId::from_uuid(parse_uuid(value)?))
}

fn lexicon_entry_id(value: &str) -> Result<LexiconEntryId, StorageError> {
    Ok(LexiconEntryId::from_uuid(parse_uuid(value)?))
}

fn lexicon_variant_id(value: &str) -> Result<LexiconVariantId, StorageError> {
    Ok(LexiconVariantId::from_uuid(parse_uuid(value)?))
}

fn transcript_id(value: &str) -> Result<TranscriptId, StorageError> {
    Ok(TranscriptId::from_uuid(parse_uuid(value)?))
}

fn transcript_correction_id(value: &str) -> Result<TranscriptCorrectionId, StorageError> {
    Ok(TranscriptCorrectionId::from_uuid(parse_uuid(value)?))
}

fn to_sql_error(error: StorageError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid migration history: {count} recorded rows, maximum version {current}")]
    InvalidMigrationHistory { count: i64, current: i64 },
    #[error("invalid persisted or requested record: {0}")]
    InvalidRecord(&'static str),
    #[error("invalid persisted or requested record: {0}")]
    InvalidRecordDetail(String),
    #[error("invalid timestamp: {0}")]
    Time(String),
    #[error("{0} was not found")]
    NotFound(&'static str),
    #[error("invalid persisted correction: {0}")]
    InvalidCorrection(String),
    #[error("search must contain 1 to 512 printable characters")]
    InvalidSearch,
    #[error("lexicon import row {row}: {message}")]
    ImportParse { row: usize, message: String },
    #[error("lexicon import validation failed: {0:?}")]
    ImportValidation(Vec<ImportValidationIssue>),
}

impl From<LexiconValidationError> for StorageError {
    fn from(value: LexiconValidationError) -> Self {
        Self::InvalidRecordDetail(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use free_whisper_domain::{InjectionOutcome, LexiconVariantId};
    use free_whisper_lexicon::{CorrectionRule, apply_corrections};
    use tempfile::tempdir;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-08-21 12:00 UTC);

    fn entry_and_variant() -> (LexiconEntry, LexiconVariant) {
        let entry = LexiconEntry::new(
            "OpenAI",
            Some("de"),
            Some("Firma"),
            80,
            LexiconScope::Global,
            NOW,
        )
        .expect("valid entry");
        let variant = LexiconVariant::new(entry.id, "open ai", MatchMode::ExactCasefold, NOW)
            .expect("valid variant");
        (entry, variant)
    }

    #[test]
    fn migrations_are_atomic_and_idempotent() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("app.sqlite3");
        let database = Database::open(&path).expect("database opens");
        assert_eq!(database.schema_version(), LATEST_SCHEMA_VERSION);
        drop(database);

        let reopened = Database::open(&path).expect("database reopens");
        assert_eq!(reopened.schema_version(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn settings_and_profiles_do_not_store_secrets() {
        let mut database = Database::in_memory().expect("database");
        database
            .settings()
            .set("language", &serde_json::json!("de"), NOW)
            .expect("setting saves");
        assert_eq!(
            database.settings().get("language").expect("setting reads"),
            Some(serde_json::json!("de"))
        );
        assert!(matches!(
            database.settings().set(
                "remote_token",
                &serde_json::json!({"value": "must-not-persist"}),
                NOW
            ),
            Err(StorageError::InvalidRecord(_))
        ));
        let profile = ProviderProfileRecord {
            id: ProviderProfileId::new(),
            provider_id: "remote-worker".to_owned(),
            display_name: "Eigener Rechner".to_owned(),
            endpoint: Some("https://worker.example.test".to_owned()),
            credential_reference: Some("free-whisper/provider/123".to_owned()),
            settings: ProviderProfileSettings {
                connect_timeout_ms: 5_000,
                request_timeout_ms: 30_000,
                developer_allow_http_loopback: false,
                remote_model_id: Some("whisper.cpp/tiny".to_owned()),
            },
            enabled: true,
            created_at: NOW,
            updated_at: NOW,
        };
        database.providers().save(&profile).expect("profile saves");
        assert_eq!(
            database.providers().get(profile.id).expect("profile reads"),
            Some(profile)
        );
        assert_eq!(database.providers().list().expect("profiles list").len(), 1);
    }

    #[test]
    fn existing_provider_profiles_gain_no_remote_model_by_default() {
        let settings: ProviderProfileSettings = serde_json::from_value(serde_json::json!({
            "connect_timeout_ms": 5_000,
            "request_timeout_ms": 30_000,
            "developer_allow_http_loopback": false
        }))
        .expect("older profile settings stay readable");
        assert_eq!(settings.remote_model_id, None);
    }

    #[test]
    fn import_validates_before_transaction_and_reports_duplicates() {
        let mut database = Database::in_memory().expect("database");
        let input = r#"canonical_text,language,category,priority,enabled,scope,profile_id,variant_text,match_mode
OpenAI,de,Firma,80,true,global,,open ai,exact_casefold
OpenAI,de,Firma,80,true,global,,Open AI,exact_casefold
"#;
        let drafts = parse_lexicon_csv(input).expect("CSV parses");
        let report = database
            .lexicon()
            .import(&drafts, NOW)
            .expect("import succeeds");
        assert_eq!(report.added_entries, 1);
        assert_eq!(report.added_variants, 1);
        assert_eq!(report.duplicate_variants, 1);

        let invalid = vec![LexiconEntryDraft {
            canonical_text: "   ".to_owned(),
            language: None,
            category: None,
            priority: 10,
            enabled: true,
            scope: LexiconScope::Global,
            variants: Vec::new(),
        }];
        assert!(matches!(
            database.lexicon().import(&invalid, NOW),
            Err(StorageError::ImportValidation(_))
        ));
        assert_eq!(database.lexicon().entries().expect("entries list").len(), 1);
    }

    #[test]
    fn transcript_revert_rebuilds_final_text_from_raw_text() {
        let mut database = Database::in_memory().expect("database");
        let (entry, variant) = entry_and_variant();
        let raw = "open ai und OPEN AI";
        let correction_result = apply_corrections(
            raw,
            &[CorrectionRule {
                entry,
                variant: variant.clone(),
            }],
            NOW,
        )
        .expect("corrections apply");
        let transcript = StoredTranscript {
            id: TranscriptId::new(),
            created_at: NOW,
            raw_text: raw.to_owned(),
            final_text: correction_result.final_text,
            language_requested: Some("de".to_owned()),
            language_detected: Some("de".to_owned()),
            language_confidence_milli: Some(900),
            provider_id: "fake".to_owned(),
            model_id: ModelId::parse("test-model").expect("model id"),
            audio_duration_ms: 1_000,
            processing_ms: 100,
            injection_outcome: InjectionOutcome::CopiedToClipboard,
            target_window: None,
            provider_metadata: None,
            segments: Vec::new(),
            warnings: Vec::new(),
            durations: ProcessingDurations::default(),
            corrections: correction_result
                .corrections
                .into_iter()
                .map(|correction| StoredCorrection {
                    correction,
                    reverted_at: None,
                })
                .collect(),
        };
        let reverted_id = transcript.corrections[0].correction.id;
        database
            .transcripts()
            .save(&transcript)
            .expect("transcript saves");

        let rebuilt = database
            .transcripts()
            .revert_correction(transcript.id, reverted_id, NOW)
            .expect("revert succeeds");

        assert_eq!(rebuilt, "open ai und OpenAI");
        let stored = database
            .transcripts()
            .get(transcript.id)
            .expect("transcript reads")
            .expect("exists");
        assert_eq!(stored.corrections.len(), 2);
        assert_eq!(
            stored
                .corrections
                .iter()
                .filter(|correction| correction.reverted_at.is_some())
                .count(),
            1
        );
        assert_eq!(
            database
                .transcripts()
                .search("OpenAI", 10)
                .expect("literal search succeeds")
                .len(),
            1
        );
        assert!(
            database
                .transcripts()
                .search("%", 10)
                .expect("wildcard is escaped")
                .is_empty()
        );
        assert!(matches!(
            database.transcripts().search("\0", 10),
            Err(StorageError::InvalidSearch)
        ));
    }

    #[test]
    fn injection_warnings_are_appended_without_rewriting_transcript_text() {
        let mut database = Database::in_memory().expect("database");
        let transcript = StoredTranscript {
            id: TranscriptId::new(),
            created_at: NOW,
            raw_text: "plain source".to_owned(),
            final_text: "plain source".to_owned(),
            language_requested: None,
            language_detected: None,
            language_confidence_milli: None,
            provider_id: "fake".to_owned(),
            model_id: ModelId::parse("test-model").expect("model id"),
            audio_duration_ms: 1,
            processing_ms: 1,
            injection_outcome: InjectionOutcome::Inserted,
            target_window: Some(TargetWindowSnapshot {
                window_handle: 11,
                process_id: 22,
                process_started_at_filetime: 33,
                title: "Notepad".to_owned(),
            }),
            provider_metadata: None,
            segments: Vec::new(),
            warnings: Vec::new(),
            durations: ProcessingDurations::default(),
            corrections: Vec::new(),
        };
        database
            .transcripts()
            .save(&transcript)
            .expect("save transcript");
        database
            .transcripts()
            .append_warning(transcript.id, "Clipboard changed; restore skipped.")
            .expect("append warning");
        let stored = database
            .transcripts()
            .get(transcript.id)
            .expect("read transcript")
            .expect("transcript exists");
        assert_eq!(stored.raw_text, "plain source");
        assert_eq!(stored.final_text, "plain source");
        assert_eq!(stored.warnings, vec!["Clipboard changed; restore skipped."]);
        assert_eq!(stored.target_window, transcript.target_window);
    }

    #[test]
    fn model_and_migration_audit_repositories_round_trip() {
        let mut database = Database::in_memory().expect("database");
        let model = InstalledModelRecord {
            model_id: ModelId::parse("ggml-base-q5_1").expect("model id"),
            provider_id: "local-whisper-cpp".to_owned(),
            installation_path: "C:/models/base.bin".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 42,
            backend: "cpu".to_owned(),
            installed_at: NOW,
            validated_at: Some(NOW),
            active: true,
        };
        database.models().save(&model).expect("model saves");
        assert_eq!(
            database.models().get(&model.model_id).expect("model reads"),
            Some(model)
        );
        assert_eq!(database.models().list().expect("models list").len(), 1);
        let audit = MigrationAuditRecord {
            source_kind: "legacy-v0".to_owned(),
            source_fingerprint: "sha256:abc".to_owned(),
            started_at: NOW,
            completed_at: Some(NOW),
            report: serde_json::json!({"imported": 0}),
        };
        database
            .migration_audit()
            .record(&audit)
            .expect("audit saves");
        assert!(
            database
                .migration_audit()
                .already_imported("legacy-v0", "sha256:abc")
                .expect("audit reads")
        );
    }

    #[test]
    fn local_model_activation_is_atomic_and_preserves_a_valid_selection() {
        let mut database = Database::in_memory().expect("database");
        let first = InstalledModelRecord {
            model_id: ModelId::parse("whisper.cpp/tiny").expect("model id"),
            provider_id: "local-whisper-cpp".to_owned(),
            installation_path: "C:/models/tiny.bin".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            backend: "cpu".to_owned(),
            installed_at: NOW,
            validated_at: Some(NOW),
            active: true,
        };
        let second = InstalledModelRecord {
            model_id: ModelId::parse("whisper.cpp/base").expect("model id"),
            installation_path: "C:/models/base.bin".to_owned(),
            active: false,
            ..first.clone()
        };
        database.models().save(&first).expect("first saves");
        database.models().save(&second).expect("second saves");
        database
            .models()
            .activate_local_cpu(&second.model_id)
            .expect("second activates");
        assert_eq!(
            database
                .models()
                .active_local_cpu()
                .expect("active model")
                .expect("one active model")
                .model_id,
            second.model_id
        );
        let invalid = InstalledModelRecord {
            model_id: ModelId::parse("whisper.cpp/invalid").expect("model id"),
            validated_at: None,
            ..second.clone()
        };
        database.models().save(&invalid).expect("invalid saves");
        assert!(
            database
                .models()
                .activate_local_cpu(&invalid.model_id)
                .is_err()
        );
        assert_eq!(
            database
                .models()
                .active_local_cpu()
                .expect("active model")
                .expect("one active model")
                .model_id,
            second.model_id
        );
    }

    #[test]
    fn malformed_csv_is_rejected_without_a_transaction() {
        let error = parse_lexicon_csv(
            "canonical_text,language,category,priority,enabled,scope,profile_id,variant_text,match_mode\nOpenAI,de,,80,true,global,,only-text,\n",
        )
        .expect_err("incomplete variant must fail");
        assert!(matches!(error, StorageError::ImportParse { .. }));
        assert_ne!(LexiconVariantId::new(), LexiconVariantId::new());
    }

    #[test]
    fn json_import_uses_the_same_validated_draft_format() {
        let drafts = parse_lexicon_json(
            r#"[
                {
                    "canonical_text": "Svelte",
                    "language": "de",
                    "category": null,
                    "priority": 60,
                    "enabled": true,
                    "scope": {"kind": "global"},
                    "variants": [
                        {"variant_text": "swelte", "match_mode": "exact_casefold"}
                    ]
                }
            ]"#,
        )
        .expect("JSON parses");
        let mut database = Database::in_memory().expect("database");
        let report = database
            .lexicon()
            .import(&drafts, NOW)
            .expect("JSON imports");
        assert_eq!(report.added_entries, 1);
        assert_eq!(report.added_variants, 1);
    }

    #[test]
    fn lexicon_profile_and_entry_deletion_are_explicit_and_cascade_variants() {
        let mut database = Database::in_memory().expect("database");
        let profile = LexiconProfile::new("Arbeit", NOW).expect("profile");
        database
            .lexicon()
            .save_profile(&profile)
            .expect("profile saves");
        let entry = LexiconEntry::new(
            "free-whisper",
            Some("de"),
            None,
            80,
            LexiconScope::Profile(profile.id),
            NOW,
        )
        .expect("entry");
        let variant = LexiconVariant::new(entry.id, "free whisper", MatchMode::ExactCasefold, NOW)
            .expect("variant");
        database
            .lexicon()
            .save_entry(&entry, &[variant])
            .expect("entry saves");

        database
            .lexicon()
            .delete_profile(profile.id)
            .expect("profile deletion succeeds");
        assert!(database.lexicon().entries().expect("entries").is_empty());
        assert!(matches!(
            database.lexicon().delete_entry(entry.id),
            Err(StorageError::NotFound("lexicon entry"))
        ));
    }
}
