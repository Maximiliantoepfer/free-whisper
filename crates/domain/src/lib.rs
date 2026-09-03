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
}
