#![forbid(unsafe_code)]

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

uuid_id!(JobId);
uuid_id!(RequestId);
uuid_id!(ProviderProfileId);
uuid_id!(LexiconProfileId);
uuid_id!(TranscriptId);
uuid_id!(TranscriptCorrectionId);
uuid_id!(LexiconEntryId);
uuid_id!(LexiconVariantId);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ModelId(String);

impl ModelId {
    pub fn parse(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IdentifierError::Empty);
        }
        if value.len() > 128 || value.chars().any(char::is_whitespace) {
            return Err(IdentifierError::Invalid(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum IdentifierError {
    #[error("identifier must not be empty")]
    Empty,
    #[error("invalid identifier: {0}")]
    Invalid(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    Idle,
    PreparingRecording,
    Recording,
    FinalizingAudio,
    Queued,
    LoadingModel,
    Transcribing,
    ApplyingCorrections,
    AwaitingInjectionConfirmation,
    Injecting,
    Completed,
    Failed,
    Cancelling,
}

impl AppState {
    #[must_use]
    pub const fn allows_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Idle, Self::PreparingRecording)
                | (
                    Self::PreparingRecording,
                    Self::Recording | Self::Cancelling | Self::Failed
                )
                | (
                    Self::Recording,
                    Self::FinalizingAudio | Self::Cancelling | Self::Failed
                )
                | (
                    Self::FinalizingAudio,
                    Self::Queued | Self::Cancelling | Self::Failed
                )
                | (
                    Self::Queued,
                    Self::LoadingModel | Self::Transcribing | Self::Cancelling | Self::Failed
                )
                | (
                    Self::LoadingModel,
                    Self::Transcribing | Self::Cancelling | Self::Failed
                )
                | (
                    Self::Transcribing,
                    Self::ApplyingCorrections | Self::Cancelling | Self::Failed
                )
                | (
                    Self::ApplyingCorrections,
                    Self::AwaitingInjectionConfirmation
                        | Self::Injecting
                        | Self::Completed
                        | Self::Failed
                )
                | (
                    Self::AwaitingInjectionConfirmation,
                    Self::Injecting | Self::Completed | Self::Cancelling | Self::Failed
                )
                | (Self::Injecting, Self::Completed | Self::Failed)
                | (Self::Completed, Self::Idle)
                | (Self::Failed, Self::Idle)
                | (Self::Cancelling, Self::Failed | Self::Idle)
        )
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StateTransitionError {
    #[error("job {job_id} cannot be transitioned by job {attempted_by}")]
    WrongJob { job_id: JobId, attempted_by: JobId },
    #[error("invalid transition for job {job_id}: {from:?} -> {to:?}")]
    Invalid {
        job_id: JobId,
        from: AppState,
        to: AppState,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobStateMachine {
    job_id: JobId,
    state: AppState,
}

impl JobStateMachine {
    #[must_use]
    pub const fn new(job_id: JobId) -> Self {
        Self {
            job_id,
            state: AppState::Idle,
        }
    }

    #[must_use]
    pub const fn job_id(&self) -> JobId {
        self.job_id
    }

    #[must_use]
    pub const fn state(&self) -> AppState {
        self.state
    }

    pub fn transition_for(
        &mut self,
        requested_by: JobId,
        next: AppState,
    ) -> Result<(), StateTransitionError> {
        if requested_by != self.job_id {
            return Err(StateTransitionError::WrongJob {
                job_id: self.job_id,
                attempted_by: requested_by,
            });
        }
        self.transition(next)
    }

    pub fn transition(&mut self, next: AppState) -> Result<(), StateTransitionError> {
        if !self.state.allows_transition_to(next) {
            return Err(StateTransitionError::Invalid {
                job_id: self.job_id,
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct CancellationSignal(Arc<AtomicBool>);

impl CancellationSignal {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioMetadata {
    pub duration_ms: u64,
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub speech_duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TargetWindowSnapshot {
    pub window_handle: u64,
    pub process_id: u32,
    pub process_started_at_filetime: u64,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderProfileSnapshot {
    pub id: ProviderProfileId,
    pub provider_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LexiconSnapshot {
    pub profile_id: Option<LexiconProfileId>,
    pub hint_terms: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TranscriptionJob {
    pub id: JobId,
    pub created_at: OffsetDateTime,
    pub recorded_at: OffsetDateTime,
    pub audio: AudioMetadata,
    pub target_window: Option<TargetWindowSnapshot>,
    pub provider_profile: ProviderProfileSnapshot,
    pub model_id: ModelId,
    pub language: LanguageSelection,
    pub lexicon: LexiconSnapshot,
    pub status: JobStateMachine,
    pub cancellation: CancellationSignal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageSelection {
    Auto,
    Explicit(String),
}

/// A language understood by the pinned multilingual whisper.cpp models.
///
/// The catalogue is intentionally code-owned instead of being accepted from
/// arbitrary client input. It mirrors the `g_lang` table in whisper.cpp
/// `23ee035`; an explicit selection is therefore safe to pass to the local
/// engine and to a compatible v1 worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WhisperLanguage {
    pub code: &'static str,
    pub display_name: &'static str,
    engine_name: &'static str,
}

const WHISPER_LANGUAGES: &[WhisperLanguage] = &[
    WhisperLanguage { code: "en", display_name: "English", engine_name: "english" },
    WhisperLanguage { code: "zh", display_name: "Chinese", engine_name: "chinese" },
    WhisperLanguage { code: "de", display_name: "Deutsch", engine_name: "german" },
    WhisperLanguage { code: "es", display_name: "Spanish", engine_name: "spanish" },
    WhisperLanguage { code: "ru", display_name: "Russian", engine_name: "russian" },
    WhisperLanguage { code: "ko", display_name: "Korean", engine_name: "korean" },
    WhisperLanguage { code: "fr", display_name: "French", engine_name: "french" },
    WhisperLanguage { code: "ja", display_name: "Japanese", engine_name: "japanese" },
    WhisperLanguage { code: "pt", display_name: "Portuguese", engine_name: "portuguese" },
    WhisperLanguage { code: "tr", display_name: "Turkish", engine_name: "turkish" },
    WhisperLanguage { code: "pl", display_name: "Polish", engine_name: "polish" },
    WhisperLanguage { code: "ca", display_name: "Catalan", engine_name: "catalan" },
    WhisperLanguage { code: "nl", display_name: "Dutch", engine_name: "dutch" },
    WhisperLanguage { code: "ar", display_name: "Arabic", engine_name: "arabic" },
    WhisperLanguage { code: "sv", display_name: "Swedish", engine_name: "swedish" },
    WhisperLanguage { code: "it", display_name: "Italian", engine_name: "italian" },
    WhisperLanguage { code: "id", display_name: "Indonesian", engine_name: "indonesian" },
    WhisperLanguage { code: "hi", display_name: "Hindi", engine_name: "hindi" },
    WhisperLanguage { code: "fi", display_name: "Finnish", engine_name: "finnish" },
    WhisperLanguage { code: "vi", display_name: "Vietnamese", engine_name: "vietnamese" },
    WhisperLanguage { code: "he", display_name: "Hebrew", engine_name: "hebrew" },
    WhisperLanguage { code: "uk", display_name: "Ukrainian", engine_name: "ukrainian" },
    WhisperLanguage { code: "el", display_name: "Greek", engine_name: "greek" },
    WhisperLanguage { code: "ms", display_name: "Malay", engine_name: "malay" },
    WhisperLanguage { code: "cs", display_name: "Czech", engine_name: "czech" },
    WhisperLanguage { code: "ro", display_name: "Romanian", engine_name: "romanian" },
    WhisperLanguage { code: "da", display_name: "Danish", engine_name: "danish" },
    WhisperLanguage { code: "hu", display_name: "Hungarian", engine_name: "hungarian" },
    WhisperLanguage { code: "ta", display_name: "Tamil", engine_name: "tamil" },
    WhisperLanguage { code: "no", display_name: "Norwegian", engine_name: "norwegian" },
    WhisperLanguage { code: "th", display_name: "Thai", engine_name: "thai" },
    WhisperLanguage { code: "ur", display_name: "Urdu", engine_name: "urdu" },
    WhisperLanguage { code: "hr", display_name: "Croatian", engine_name: "croatian" },
    WhisperLanguage { code: "bg", display_name: "Bulgarian", engine_name: "bulgarian" },
    WhisperLanguage { code: "lt", display_name: "Lithuanian", engine_name: "lithuanian" },
    WhisperLanguage { code: "la", display_name: "Latin", engine_name: "latin" },
    WhisperLanguage { code: "mi", display_name: "Maori", engine_name: "maori" },
    WhisperLanguage { code: "ml", display_name: "Malayalam", engine_name: "malayalam" },
    WhisperLanguage { code: "cy", display_name: "Welsh", engine_name: "welsh" },
    WhisperLanguage { code: "sk", display_name: "Slovak", engine_name: "slovak" },
    WhisperLanguage { code: "te", display_name: "Telugu", engine_name: "telugu" },
    WhisperLanguage { code: "fa", display_name: "Persian", engine_name: "persian" },
    WhisperLanguage { code: "lv", display_name: "Latvian", engine_name: "latvian" },
    WhisperLanguage { code: "bn", display_name: "Bengali", engine_name: "bengali" },
    WhisperLanguage { code: "sr", display_name: "Serbian", engine_name: "serbian" },
    WhisperLanguage { code: "az", display_name: "Azerbaijani", engine_name: "azerbaijani" },
    WhisperLanguage { code: "sl", display_name: "Slovenian", engine_name: "slovenian" },
    WhisperLanguage { code: "kn", display_name: "Kannada", engine_name: "kannada" },
    WhisperLanguage { code: "et", display_name: "Estonian", engine_name: "estonian" },
    WhisperLanguage { code: "mk", display_name: "Macedonian", engine_name: "macedonian" },
    WhisperLanguage { code: "br", display_name: "Breton", engine_name: "breton" },
    WhisperLanguage { code: "eu", display_name: "Basque", engine_name: "basque" },
    WhisperLanguage { code: "is", display_name: "Icelandic", engine_name: "icelandic" },
    WhisperLanguage { code: "hy", display_name: "Armenian", engine_name: "armenian" },
    WhisperLanguage { code: "ne", display_name: "Nepali", engine_name: "nepali" },
    WhisperLanguage { code: "mn", display_name: "Mongolian", engine_name: "mongolian" },
    WhisperLanguage { code: "bs", display_name: "Bosnian", engine_name: "bosnian" },
    WhisperLanguage { code: "kk", display_name: "Kazakh", engine_name: "kazakh" },
    WhisperLanguage { code: "sq", display_name: "Albanian", engine_name: "albanian" },
    WhisperLanguage { code: "sw", display_name: "Swahili", engine_name: "swahili" },
    WhisperLanguage { code: "gl", display_name: "Galician", engine_name: "galician" },
    WhisperLanguage { code: "mr", display_name: "Marathi", engine_name: "marathi" },
    WhisperLanguage { code: "pa", display_name: "Punjabi", engine_name: "punjabi" },
    WhisperLanguage { code: "si", display_name: "Sinhala", engine_name: "sinhala" },
    WhisperLanguage { code: "km", display_name: "Khmer", engine_name: "khmer" },
    WhisperLanguage { code: "sn", display_name: "Shona", engine_name: "shona" },
    WhisperLanguage { code: "yo", display_name: "Yoruba", engine_name: "yoruba" },
    WhisperLanguage { code: "so", display_name: "Somali", engine_name: "somali" },
    WhisperLanguage { code: "af", display_name: "Afrikaans", engine_name: "afrikaans" },
    WhisperLanguage { code: "oc", display_name: "Occitan", engine_name: "occitan" },
    WhisperLanguage { code: "ka", display_name: "Georgian", engine_name: "georgian" },
    WhisperLanguage { code: "be", display_name: "Belarusian", engine_name: "belarusian" },
    WhisperLanguage { code: "tg", display_name: "Tajik", engine_name: "tajik" },
    WhisperLanguage { code: "sd", display_name: "Sindhi", engine_name: "sindhi" },
    WhisperLanguage { code: "gu", display_name: "Gujarati", engine_name: "gujarati" },
    WhisperLanguage { code: "am", display_name: "Amharic", engine_name: "amharic" },
    WhisperLanguage { code: "yi", display_name: "Yiddish", engine_name: "yiddish" },
    WhisperLanguage { code: "lo", display_name: "Lao", engine_name: "lao" },
    WhisperLanguage { code: "uz", display_name: "Uzbek", engine_name: "uzbek" },
    WhisperLanguage { code: "fo", display_name: "Faroese", engine_name: "faroese" },
    WhisperLanguage { code: "ht", display_name: "Haitian Creole", engine_name: "haitian creole" },
    WhisperLanguage { code: "ps", display_name: "Pashto", engine_name: "pashto" },
    WhisperLanguage { code: "tk", display_name: "Turkmen", engine_name: "turkmen" },
    WhisperLanguage { code: "nn", display_name: "Nynorsk", engine_name: "nynorsk" },
    WhisperLanguage { code: "mt", display_name: "Maltese", engine_name: "maltese" },
    WhisperLanguage { code: "sa", display_name: "Sanskrit", engine_name: "sanskrit" },
    WhisperLanguage { code: "lb", display_name: "Luxembourgish", engine_name: "luxembourgish" },
    WhisperLanguage { code: "my", display_name: "Myanmar", engine_name: "myanmar" },
    WhisperLanguage { code: "bo", display_name: "Tibetan", engine_name: "tibetan" },
    WhisperLanguage { code: "tl", display_name: "Tagalog", engine_name: "tagalog" },
    WhisperLanguage { code: "mg", display_name: "Malagasy", engine_name: "malagasy" },
    WhisperLanguage { code: "as", display_name: "Assamese", engine_name: "assamese" },
    WhisperLanguage { code: "tt", display_name: "Tatar", engine_name: "tatar" },
    WhisperLanguage { code: "haw", display_name: "Hawaiian", engine_name: "hawaiian" },
    WhisperLanguage { code: "ln", display_name: "Lingala", engine_name: "lingala" },
    WhisperLanguage { code: "ha", display_name: "Hausa", engine_name: "hausa" },
    WhisperLanguage { code: "ba", display_name: "Bashkir", engine_name: "bashkir" },
    WhisperLanguage { code: "jw", display_name: "Javanese", engine_name: "javanese" },
    WhisperLanguage { code: "su", display_name: "Sundanese", engine_name: "sundanese" },
    WhisperLanguage { code: "yue", display_name: "Cantonese", engine_name: "cantonese" },
];

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("unsupported whisper language: {0}")]
pub struct LanguageSelectionError(String);

#[must_use]
pub const fn supported_whisper_languages() -> &'static [WhisperLanguage] {
    WHISPER_LANGUAGES
}

pub fn normalize_language_selection(value: Option<&str>) -> Result<LanguageSelection, LanguageSelectionError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(LanguageSelection::Auto);
    };
    if value.eq_ignore_ascii_case("auto") {
        return Ok(LanguageSelection::Auto);
    }
    let normalized = value.to_ascii_lowercase();
    WHISPER_LANGUAGES
        .iter()
        .find(|language| normalized == language.code || normalized == language.engine_name)
        .map(|language| LanguageSelection::Explicit(language.code.to_owned()))
        .ok_or_else(|| LanguageSelectionError(value.to_owned()))
}

#[must_use]
pub fn language_display_name(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized == "auto" {
        return "Automatisch".to_owned();
    }
    WHISPER_LANGUAGES
        .iter()
        .find(|language| normalized == language.code || normalized == language.engine_name)
        .map_or_else(|| value.to_owned(), |language| language.display_name.to_owned())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PcmF32Mono {
    pub sample_rate_hz: u32,
    pub samples: Vec<f32>,
}

impl PcmF32Mono {
    pub fn new(sample_rate_hz: u32, samples: Vec<f32>) -> Result<Self, AudioPayloadError> {
        if sample_rate_hz == 0 {
            return Err(AudioPayloadError::ZeroSampleRate);
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(AudioPayloadError::NonFiniteSample);
        }
        Ok(Self {
            sample_rate_hz,
            samples,
        })
    }

    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        let samples = u64::try_from(self.samples.len()).unwrap_or(u64::MAX);
        samples.saturating_mul(1_000) / u64::from(self.sample_rate_hz)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AudioPayloadError {
    #[error("sample rate must be greater than zero")]
    ZeroSampleRate,
    #[error("audio contains a non-finite sample")]
    NonFiniteSample,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DecodingOptions {
    pub temperature: f32,
    pub beam_size: u8,
    pub word_timestamps: bool,
}

impl Default for DecodingOptions {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            beam_size: 5,
            word_timestamps: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TranscriptionRequest {
    pub id: RequestId,
    pub audio: PcmF32Mono,
    pub language: LanguageSelection,
    pub decoding: DecodingOptions,
    pub prompt_terms: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppliedCorrection {
    pub id: TranscriptCorrectionId,
    pub variant_id: LexiconVariantId,
    pub original: String,
    pub replacement: String,
    pub start_char: u32,
    pub end_char: u32,
    pub applied_at: OffsetDateTime,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessingDurations {
    pub queue_ms: u64,
    pub model_load_ms: u64,
    pub inference_ms: u64,
    pub correction_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderMetadata {
    pub provider_id: String,
    pub model_id: ModelId,
    pub backend: ExecutionBackend,
    pub quantization: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptionResult {
    pub raw_text: String,
    pub final_text: String,
    pub language_detected: Option<String>,
    pub language_confidence_milli: Option<u16>,
    pub segments: Vec<TranscriptSegment>,
    pub provider: ProviderMetadata,
    pub durations: ProcessingDurations,
    pub warnings: Vec<String>,
    pub corrections: Vec<AppliedCorrection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    Cpu,
    Vulkan,
    Cuda,
    OpenVino,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GpuInformation {
    pub backend: ExecutionBackend,
    pub name: String,
    pub available: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderCapabilities {
    pub local_execution: bool,
    pub remote_execution: bool,
    pub streaming: bool,
    pub vad: bool,
    pub word_timestamps: bool,
    pub model_switching: bool,
    pub language_detection: bool,
    pub gpu: Option<GpuInformation>,
    pub max_request_bytes: u64,
    pub max_audio_duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionOutcome {
    Inserted,
    CopiedToClipboard,
    NeedsUserConfirmation { reason: String },
    FailedWithReason { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_rejects_invalid_transition() {
        let mut machine = JobStateMachine::new(JobId::new());

        let result = machine.transition(AppState::Transcribing);

        assert!(matches!(result, Err(StateTransitionError::Invalid { .. })));
        assert_eq!(machine.state(), AppState::Idle);
    }

    #[test]
    fn state_machine_rejects_other_job() {
        let owner = JobId::new();
        let mut machine = JobStateMachine::new(owner);

        let result = machine.transition_for(JobId::new(), AppState::PreparingRecording);

        assert!(matches!(result, Err(StateTransitionError::WrongJob { .. })));
        assert_eq!(machine.state(), AppState::Idle);
    }

    #[test]
    fn state_machine_allows_recording_flow() {
        let mut machine = JobStateMachine::new(JobId::new());
        for state in [
            AppState::PreparingRecording,
            AppState::Recording,
            AppState::FinalizingAudio,
            AppState::Queued,
            AppState::LoadingModel,
            AppState::Transcribing,
            AppState::ApplyingCorrections,
            AppState::AwaitingInjectionConfirmation,
            AppState::Injecting,
            AppState::Completed,
            AppState::Idle,
        ] {
            machine.transition(state).expect("valid transition");
        }
    }

    #[test]
    fn pcm_rejects_non_finite_samples() {
        let result = PcmF32Mono::new(16_000, vec![0.0, f32::NAN]);

        assert_eq!(result, Err(AudioPayloadError::NonFiniteSample));
    }

    #[test]
    fn whisper_language_selection_is_normalized_against_the_pinned_catalogue() {
        assert_eq!(
            normalize_language_selection(None).expect("auto is valid"),
            LanguageSelection::Auto
        );
        assert_eq!(
            normalize_language_selection(Some("german")).expect("engine alias is valid"),
            LanguageSelection::Explicit("de".to_owned())
        );
        assert_eq!(language_display_name("de"), "Deutsch");
        assert_eq!(supported_whisper_languages().len(), 100);
        assert!(normalize_language_selection(Some("not-a-language")).is_err());
    }
}
