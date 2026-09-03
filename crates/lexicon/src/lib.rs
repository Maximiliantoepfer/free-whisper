#![forbid(unsafe_code)]

//! Deterministic prompt selection and deliberately conservative lexicon corrections.

use std::{cmp::Reverse, collections::BTreeSet};

use caseless::default_case_fold_str;
use free_whisper_domain::{
    AppliedCorrection, LexiconEntryId, LexiconProfileId, LexiconVariantId, TranscriptCorrectionId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

pub const MAX_PROMPT_TERMS: usize = 32;
pub const MAX_PROMPT_TOKENS: usize = 128;
const MAX_TERM_CHARS: usize = 256;
const MAX_CATEGORY_CHARS: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "profile_id", rename_all = "snake_case")]
pub enum LexiconScope {
    Global,
    Profile(LexiconProfileId),
}

impl LexiconScope {
    #[must_use]
    pub fn applies_to(&self, active_profile: Option<LexiconProfileId>) -> bool {
        match self {
            Self::Global => true,
            Self::Profile(profile_id) => Some(*profile_id) == active_profile,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LexiconProfile {
    pub id: LexiconProfileId,
    pub name: String,
    pub normalized_name: String,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl LexiconProfile {
    pub fn new(name: &str, now: OffsetDateTime) -> Result<Self, LexiconValidationError> {
        let name = normalize_display_text(name, "profile name")?;
        Ok(Self {
            id: LexiconProfileId::new(),
            normalized_name: casefold_key(&name),
            name,
            enabled: true,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LexiconEntry {
    pub id: LexiconEntryId,
    pub canonical_text: String,
    pub canonical_key: String,
    pub language: Option<String>,
    pub category: Option<String>,
    pub priority: u8,
    pub enabled: bool,
    pub scope: LexiconScope,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl LexiconEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        canonical_text: &str,
        language: Option<&str>,
        category: Option<&str>,
        priority: u8,
        scope: LexiconScope,
        now: OffsetDateTime,
    ) -> Result<Self, LexiconValidationError> {
        if priority > 100 {
            return Err(LexiconValidationError::InvalidPriority(priority));
        }

        let canonical_text = normalize_term(canonical_text, "canonical text")?;
        let language = normalize_language(language)?;
        let category = normalize_optional(category, "category", MAX_CATEGORY_CHARS)?;
        Ok(Self {
            id: LexiconEntryId::new(),
            canonical_key: casefold_key(&canonical_text),
            canonical_text,
            language,
            category,
            priority,
            enabled: true,
            scope,
            created_at: now,
            updated_at: now,
        })
    }

    #[must_use]
    pub fn identity_key(&self) -> String {
        let scope = match self.scope {
            LexiconScope::Global => "global".to_owned(),
            LexiconScope::Profile(profile_id) => format!("profile:{profile_id}"),
        };
        format!(
            "{scope}|{}|{}",
            self.language.as_deref().unwrap_or_default(),
            self.canonical_key
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    ExactCasefold,
    ExactCaseSensitive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LexiconVariant {
    pub id: LexiconVariantId,
    pub entry_id: LexiconEntryId,
    pub variant_text: String,
    pub variant_key: String,
    pub match_mode: MatchMode,
    pub created_at: OffsetDateTime,
}

impl LexiconVariant {
    pub fn new(
        entry_id: LexiconEntryId,
        variant_text: &str,
        match_mode: MatchMode,
        now: OffsetDateTime,
    ) -> Result<Self, LexiconValidationError> {
        let variant_text = normalize_term(variant_text, "variant text")?;
        let variant_key = match match_mode {
            MatchMode::ExactCasefold => casefold_key(&variant_text),
            MatchMode::ExactCaseSensitive => variant_text.clone(),
        };
        Ok(Self {
            id: LexiconVariantId::new(),
            entry_id,
            variant_text,
            variant_key,
            match_mode,
            created_at: now,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LexiconEntryDraft {
    pub canonical_text: String,
    pub language: Option<String>,
    pub category: Option<String>,
    pub priority: u8,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub scope: LexiconScope,
    pub variants: Vec<LexiconVariantDraft>,
}

impl LexiconEntryDraft {
    pub fn validate(
        &self,
        now: OffsetDateTime,
    ) -> Result<ValidatedEntryDraft, LexiconValidationError> {
        let mut entry = LexiconEntry::new(
            &self.canonical_text,
            self.language.as_deref(),
            self.category.as_deref(),
            self.priority,
            self.scope.clone(),
            now,
        )?;
        entry.enabled = self.enabled;
        let mut variants = Vec::with_capacity(self.variants.len());
        for variant in &self.variants {
            let value =
                LexiconVariant::new(entry.id, &variant.variant_text, variant.match_mode, now)?;
            variants.push(value);
        }
        Ok(ValidatedEntryDraft { entry, variants })
    }
}

const fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LexiconVariantDraft {
    pub variant_text: String,
    pub match_mode: MatchMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedEntryDraft {
    pub entry: LexiconEntry,
    pub variants: Vec<LexiconVariant>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum LexiconValidationError {
    #[error("{field} must not be empty")]
    Empty { field: &'static str },
    #[error("{field} exceeds {max} characters")]
    TooLong { field: &'static str, max: usize },
    #[error("language must use a conservative BCP-47-like identifier")]
    InvalidLanguage,
    #[error("priority {0} is outside the supported range 0..=100")]
    InvalidPriority(u8),
    #[error("duplicate variant after normalisation: {0}")]
    DuplicateVariant(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptSelection {
    pub terms: Vec<String>,
    pub estimated_tokens: usize,
}

#[must_use]
pub fn select_prompt_terms(
    entries: &[LexiconEntry],
    active_profile: Option<LexiconProfileId>,
) -> PromptSelection {
    let mut candidates = entries
        .iter()
        .filter(|entry| entry.enabled && entry.scope.applies_to(active_profile))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|entry| {
        (
            Reverse(entry.priority),
            entry.canonical_key.as_str(),
            entry.id.as_uuid(),
        )
    });

    let mut terms = Vec::with_capacity(MAX_PROMPT_TERMS);
    let mut estimated_tokens: usize = 0;
    for entry in candidates {
        let term_tokens = estimate_tokens(&entry.canonical_text);
        if terms.len() == MAX_PROMPT_TERMS
            || estimated_tokens.saturating_add(term_tokens) > MAX_PROMPT_TOKENS
        {
            continue;
        }
        estimated_tokens += term_tokens;
        terms.push(entry.canonical_text.clone());
    }

    PromptSelection {
        terms,
        estimated_tokens,
    }
}

#[derive(Clone, Debug)]
pub struct CorrectionRule {
    pub entry: LexiconEntry,
    pub variant: LexiconVariant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrectionResult {
    pub final_text: String,
    pub corrections: Vec<AppliedCorrection>,
}

pub fn apply_corrections(
    raw_text: &str,
    rules: &[CorrectionRule],
    applied_at: OffsetDateTime,
) -> Result<CorrectionResult, CorrectionError> {
    let boundary_offsets = word_boundary_offsets(raw_text);
    let folded = FoldedText::new(raw_text);
    let mut candidates = Vec::new();

    for rule in rules {
        if rule.variant.entry_id != rule.entry.id {
            return Err(CorrectionError::VariantBelongsToOtherEntry {
                variant_id: rule.variant.id,
                entry_id: rule.entry.id,
            });
        }
        if !rule.entry.enabled || boundary_offsets.len() <= 1 {
            continue;
        }
        candidates.extend(find_candidates(raw_text, &boundary_offsets, &folded, rule));
    }

    candidates.sort_by(|left, right| {
        left.start_byte
            .cmp(&right.start_byte)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| right.end_byte.cmp(&left.end_byte))
            .then_with(|| left.replacement.cmp(&right.replacement))
            .then_with(|| left.variant_id.as_uuid().cmp(&right.variant_id.as_uuid()))
    });

    let mut selected = Vec::new();
    let mut next_free_byte = 0;
    for candidate in candidates {
        if candidate.start_byte >= next_free_byte {
            next_free_byte = candidate.end_byte;
            selected.push(candidate);
        }
    }

    let mut final_text = String::with_capacity(raw_text.len());
    let mut corrections = Vec::with_capacity(selected.len());
    let mut cursor = 0;
    for candidate in selected {
        final_text.push_str(&raw_text[cursor..candidate.start_byte]);
        final_text.push_str(&candidate.replacement);
        corrections.push(AppliedCorrection {
            id: TranscriptCorrectionId::new(),
            variant_id: candidate.variant_id,
            original: raw_text[candidate.start_byte..candidate.end_byte].to_owned(),
            replacement: candidate.replacement,
            start_char: char_count_to_u32(raw_text, candidate.start_byte)?,
            end_char: char_count_to_u32(raw_text, candidate.end_byte)?,
            applied_at,
        });
        cursor = candidate.end_byte;
    }
    final_text.push_str(&raw_text[cursor..]);

    Ok(CorrectionResult {
        final_text,
        corrections,
    })
}

/// Rebuilds final text from immutable raw text and the not-reverted corrections.
pub fn render_corrections(
    raw_text: &str,
    corrections: &[AppliedCorrection],
) -> Result<String, CorrectionError> {
    let mut ordered = corrections.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|correction| {
        (
            correction.start_char,
            correction.end_char,
            correction.id.as_uuid(),
        )
    });

    let mut output = String::with_capacity(raw_text.len());
    let mut previous_end = 0;
    for correction in ordered {
        let start = byte_offset_at_char(raw_text, correction.start_char)?;
        let end = byte_offset_at_char(raw_text, correction.end_char)?;
        if start < previous_end || start > end || raw_text[start..end] != correction.original {
            return Err(CorrectionError::InvalidStoredCorrection(correction.id));
        }
        output.push_str(&raw_text[previous_end..start]);
        output.push_str(&correction.replacement);
        previous_end = end;
    }
    output.push_str(&raw_text[previous_end..]);
    Ok(output)
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CorrectionError {
    #[error("variant {variant_id} does not belong to entry {entry_id}")]
    VariantBelongsToOtherEntry {
        variant_id: LexiconVariantId,
        entry_id: LexiconEntryId,
    },
    #[error("a correction position exceeds the supported u32 character range")]
    PositionTooLarge,
    #[error("stored correction {0} cannot be safely applied to the raw text")]
    InvalidStoredCorrection(TranscriptCorrectionId),
}

#[derive(Clone, Debug)]
struct Candidate {
    start_byte: usize,
    end_byte: usize,
    priority: u8,
    variant_id: LexiconVariantId,
    replacement: String,
}

fn find_candidates(
    raw_text: &str,
    boundaries: &BTreeSet<usize>,
    folded: &FoldedText,
    rule: &CorrectionRule,
) -> Vec<Candidate> {
    let locations = match rule.variant.match_mode {
        MatchMode::ExactCaseSensitive => raw_text
            .match_indices(&rule.variant.variant_text)
            .map(|(start_byte, matched)| (start_byte, start_byte + matched.len()))
            .collect(),
        MatchMode::ExactCasefold => folded.find_all(&rule.variant.variant_key),
    };

    locations
        .into_iter()
        .filter(|(start, end)| boundaries.contains(start) && boundaries.contains(end))
        .map(|(start_byte, end_byte)| Candidate {
            start_byte,
            end_byte,
            priority: rule.entry.priority,
            variant_id: rule.variant.id,
            replacement: rule.entry.canonical_text.clone(),
        })
        .collect()
}

#[derive(Clone, Debug)]
struct FoldSpan {
    folded_start: usize,
    folded_end: usize,
    raw_start: usize,
    raw_end: usize,
}

#[derive(Clone, Debug)]
struct FoldedText {
    value: String,
    spans: Vec<FoldSpan>,
}

impl FoldedText {
    fn new(raw_text: &str) -> Self {
        let mut value = String::with_capacity(raw_text.len());
        let mut spans = Vec::with_capacity(raw_text.chars().count());
        for (raw_start, character) in raw_text.char_indices() {
            let raw_end = raw_start + character.len_utf8();
            let folded_start = value.len();
            value.push_str(&default_case_fold_str(&character.to_string()));
            spans.push(FoldSpan {
                folded_start,
                folded_end: value.len(),
                raw_start,
                raw_end,
            });
        }
        Self { value, spans }
    }

    fn find_all(&self, needle: &str) -> Vec<(usize, usize)> {
        self.value
            .match_indices(needle)
            .filter_map(|(start, matched)| {
                let end = start + matched.len();
                let first = self.spans.iter().find(|span| span.folded_start == start)?;
                let last = self.spans.iter().find(|span| span.folded_end == end)?;
                Some((first.raw_start, last.raw_end))
            })
            .collect()
    }
}

fn word_boundary_offsets(text: &str) -> BTreeSet<usize> {
    let mut offsets = BTreeSet::from([0, text.len()]);
    offsets.extend(text.split_word_bound_indices().map(|(offset, _)| offset));
    offsets
}

fn char_count_to_u32(text: &str, byte_offset: usize) -> Result<u32, CorrectionError> {
    let count = text[..byte_offset].chars().count();
    u32::try_from(count).map_err(|_| CorrectionError::PositionTooLarge)
}

fn byte_offset_at_char(text: &str, char_offset: u32) -> Result<usize, CorrectionError> {
    let offset = usize::try_from(char_offset).map_err(|_| CorrectionError::PositionTooLarge)?;
    if offset == text.chars().count() {
        return Ok(text.len());
    }
    text.char_indices()
        .nth(offset)
        .map(|(byte_offset, _)| byte_offset)
        .ok_or(CorrectionError::PositionTooLarge)
}

fn estimate_tokens(term: &str) -> usize {
    // Whisper's tokenizer is model-specific. Two Unicode scalar values per token is a
    // deliberately conservative, deterministic upper estimate for hint selection.
    term.chars().count().div_ceil(2).max(1)
}

fn normalize_term(value: &str, field: &'static str) -> Result<String, LexiconValidationError> {
    let normalized = normalize_display_text(value, field)?;
    if normalized.chars().count() > MAX_TERM_CHARS {
        return Err(LexiconValidationError::TooLong {
            field,
            max: MAX_TERM_CHARS,
        });
    }
    Ok(normalized)
}

fn normalize_display_text(
    value: &str,
    field: &'static str,
) -> Result<String, LexiconValidationError> {
    let normalized = value
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return Err(LexiconValidationError::Empty { field });
    }
    Ok(normalized)
}

fn normalize_optional(
    value: Option<&str>,
    field: &'static str,
    max: usize,
) -> Result<Option<String>, LexiconValidationError> {
    value
        .map(|value| {
            let normalized = normalize_display_text(value, field)?;
            if normalized.chars().count() > max {
                return Err(LexiconValidationError::TooLong { field, max });
            }
            Ok(normalized)
        })
        .transpose()
}

fn normalize_language(value: Option<&str>) -> Result<Option<String>, LexiconValidationError> {
    value
        .map(|value| {
            let normalized = normalize_display_text(value, "language")?.to_ascii_lowercase();
            if normalized.len() > 35
                || !normalized
                    .bytes()
                    .all(|character| character.is_ascii_alphabetic() || character == b'-')
            {
                return Err(LexiconValidationError::InvalidLanguage);
            }
            Ok(normalized)
        })
        .transpose()
}

fn casefold_key(value: &str) -> String {
    default_case_fold_str(value).nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-08-21 12:00 UTC);

    fn entry(text: &str, priority: u8) -> LexiconEntry {
        LexiconEntry::new(
            text,
            Some("de-DE"),
            None,
            priority,
            LexiconScope::Global,
            NOW,
        )
        .expect("valid entry")
    }

    #[test]
    fn normalisation_makes_equivalent_casefold_entries_identical() {
        let composed = entry("  Café  ", 50);
        let decomposed = entry("CAFE\u{301}", 50);

        assert_eq!(composed.canonical_text, "Café");
        assert_eq!(composed.canonical_key, decomposed.canonical_key);
        assert_eq!(composed.identity_key(), decomposed.identity_key());
    }

    #[test]
    fn prompt_selection_is_scoped_deterministic_and_bounded() {
        let profile = LexiconProfileId::new();
        let mut entries = (0..40)
            .map(|number| entry(&format!("Begriff {number:02}"), 20))
            .collect::<Vec<_>>();
        let scoped = LexiconEntry::new(
            "Nur Profil",
            Some("de"),
            None,
            100,
            LexiconScope::Profile(profile),
            NOW,
        )
        .expect("valid scoped entry");
        entries.push(scoped);

        let selection = select_prompt_terms(&entries, Some(profile));

        assert_eq!(selection.terms.first(), Some(&"Nur Profil".to_owned()));
        assert!(selection.terms.len() <= MAX_PROMPT_TERMS);
        assert!(selection.estimated_tokens <= MAX_PROMPT_TOKENS);
    }

    #[test]
    fn correction_only_matches_explicit_variants_at_unicode_word_boundaries() {
        let replacement = entry("OpenAI", 80);
        let variant = LexiconVariant::new(replacement.id, "open ai", MatchMode::ExactCasefold, NOW)
            .expect("valid variant");
        let result = apply_corrections(
            "Open AI entwickelt open air, aber (OPEN AI)!",
            &[CorrectionRule {
                entry: replacement,
                variant,
            }],
            NOW,
        )
        .expect("correction succeeds");

        assert_eq!(
            result.final_text,
            "OpenAI entwickelt open air, aber (OpenAI)!"
        );
        assert_eq!(result.corrections.len(), 2);
    }

    #[test]
    fn full_casefold_does_not_allow_partial_character_matches() {
        let replacement = entry("Straße", 80);
        let variant = LexiconVariant::new(replacement.id, "STRASSE", MatchMode::ExactCasefold, NOW)
            .expect("valid variant");
        let result = apply_corrections(
            "Die straße und STRASSE, aber nicht Straßen.",
            &[CorrectionRule {
                entry: replacement,
                variant,
            }],
            NOW,
        )
        .expect("correction succeeds");

        assert_eq!(
            result.final_text,
            "Die Straße und Straße, aber nicht Straßen."
        );
        assert_eq!(result.corrections.len(), 2);
    }

    #[test]
    fn rendering_remaining_corrections_rebuilds_from_raw_text() {
        let replacement = entry("Codex", 80);
        let variant = LexiconVariant::new(replacement.id, "kodeks", MatchMode::ExactCasefold, NOW)
            .expect("valid variant");
        let raw = "kodeks und KODEKS";
        let result = apply_corrections(
            raw,
            &[CorrectionRule {
                entry: replacement,
                variant,
            }],
            NOW,
        )
        .expect("correction succeeds");

        let rebuilt = render_corrections(raw, &result.corrections[1..]).expect("safe render");

        assert_eq!(rebuilt, "kodeks und Codex");
    }
}
