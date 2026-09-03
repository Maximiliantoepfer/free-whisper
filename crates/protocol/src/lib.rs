#![forbid(unsafe_code)]

//! Versioned, transport-only data transfer objects for the worker HTTP API.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Every remote API response carries this protocol version.
pub const API_VERSION: u16 = 1;

/// Implemented by every JSON response at the public worker boundary.
/// Clients must reject a response from another major protocol version rather
/// than guessing its meaning.
pub trait VersionedResponse {
    fn api_version(&self) -> u16;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthResponseV1 {
    pub api_version: u16,
    pub status: HealthStatus,
    pub worker_instance_id: Uuid,
}

impl VersionedResponse for HealthResponseV1 {
    fn api_version(&self) -> u16 {
        self.api_version
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ready,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilitiesResponseV1 {
    pub api_version: u16,
    pub local_execution: bool,
    pub remote_execution: bool,
    pub streaming: bool,
    pub vad: bool,
    pub word_timestamps: bool,
    pub model_switching: bool,
    pub language_detection: bool,
    pub gpu: Option<GpuInfoV1>,
    pub max_request_bytes: u64,
    pub max_audio_duration_ms: u64,
}

impl VersionedResponse for CapabilitiesResponseV1 {
    fn api_version(&self) -> u16 {
        self.api_version
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackendV1 {
    Cpu,
    Vulkan,
    Cuda,
    OpenVino,
}

impl ExecutionBackendV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Vulkan => "vulkan",
            Self::Cuda => "cuda",
            Self::OpenVino => "open_vino",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GpuInfoV1 {
    pub backend: ExecutionBackendV1,
    pub name: String,
    pub available: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelsResponseV1 {
    pub api_version: u16,
    pub models: Vec<ModelInfoV1>,
}

impl VersionedResponse for ModelsResponseV1 {
    fn api_version(&self) -> u16 {
        self.api_version
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelInfoV1 {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
    pub ready: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptionOptionsV1 {
    pub request_id: Uuid,
    pub language: LanguageRequestV1,
    #[serde(default)]
    pub prompt_terms: Vec<String>,
    #[serde(default)]
    pub temperature_milli: u16,
    #[serde(default = "default_beam_size")]
    pub beam_size: u8,
    #[serde(default)]
    pub word_timestamps: bool,
}

const fn default_beam_size() -> u8 {
    5
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", content = "language", rename_all = "snake_case")]
pub enum LanguageRequestV1 {
    Auto,
    Explicit(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncodingV1 {
    Wav,
    PcmF32LeMono16Khz,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptionResponseV1 {
    pub api_version: u16,
    pub request_id: Uuid,
    pub raw_text: String,
    pub language_detected: Option<String>,
    pub language_confidence_milli: Option<u16>,
    pub segments: Vec<TranscriptSegmentV1>,
    pub metadata: InferenceMetadataV1,
    pub warnings: Vec<String>,
}

impl VersionedResponse for TranscriptionResponseV1 {
    fn api_version(&self) -> u16 {
        self.api_version
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptSegmentV1 {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InferenceMetadataV1 {
    pub provider_id: String,
    pub model_id: String,
    pub backend: ExecutionBackendV1,
    pub quantization: Option<String>,
    pub inference_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CancelResponseV1 {
    pub api_version: u16,
    pub request_id: Uuid,
    pub cancelled: bool,
}

impl VersionedResponse for CancelResponseV1 {
    fn api_version(&self) -> u16 {
        self.api_version
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiErrorResponseV1 {
    pub api_version: u16,
    pub error: ApiErrorV1,
}

impl VersionedResponse for ApiErrorResponseV1 {
    fn api_version(&self) -> u16 {
        self.api_version
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiErrorV1 {
    pub code: ApiErrorCodeV1,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCodeV1 {
    Unauthorized,
    InvalidRequest,
    RequestTooLarge,
    AudioTooLong,
    RateLimited,
    Busy,
    ModelNotReady,
    Cancelled,
    NotFound,
    EngineUnavailable,
    /// The worker could reach its managed inference engine, but that engine
    /// returned a response outside the pinned adapter contract.
    EngineProtocol,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_with_explicit_versioned_shape() {
        let options = TranscriptionOptionsV1 {
            request_id: Uuid::nil(),
            language: LanguageRequestV1::Explicit("de".to_owned()),
            prompt_terms: vec!["OpenAI".to_owned()],
            temperature_milli: 0,
            beam_size: 5,
            word_timestamps: true,
        };

        let json = serde_json::to_value(&options).expect("serialises");
        assert_eq!(json["language"]["mode"], "explicit");
        assert_eq!(json["request_id"], Uuid::nil().to_string());
        assert_eq!(
            serde_json::from_value::<TranscriptionOptionsV1>(json).expect("deserialises"),
            options
        );
    }
}
