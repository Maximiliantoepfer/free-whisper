#![forbid(unsafe_code)]

//! Stable provider trait and v1 HTTP client. Local execution uses this same
//! client against a managed loopback worker; remote execution requires HTTPS.

use std::{
    net::IpAddr,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use free_whisper_domain::{
    ExecutionBackend, ModelId, ProviderCapabilities, RequestId, TranscriptionRequest,
    TranscriptionResult,
};
use free_whisper_protocol::{
    ApiErrorCodeV1, ApiErrorResponseV1, CapabilitiesResponseV1, LanguageRequestV1,
    ModelsResponseV1, TranscriptionOptionsV1, TranscriptionResponseV1, VersionedResponse,
};
use reqwest::{StatusCode, header};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::Mutex as AsyncMutex,
};
use url::{Host, Url};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderHealth {
    pub healthy: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableModel {
    pub id: ModelId,
    pub display_name: String,
    pub installed: bool,
    pub ready: bool,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProviderError {
    #[error("provider is unavailable: {0}")]
    Unavailable(String),
    #[error("remote worker authentication failed")]
    Unauthorized,
    #[error("remote endpoint is not permitted: {0}")]
    InsecureEndpoint(String),
    #[error("model is not ready: {0}")]
    ModelNotReady(String),
    #[error("request {0} was cancelled")]
    Cancelled(RequestId),
    #[error("provider rejected the request: {0}")]
    Rejected(String),
    #[error("provider protocol error: {0}")]
    Protocol(String),
}

#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError>;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn list_models(&self) -> Result<Vec<AvailableModel>, ProviderError>;
    async fn ensure_model_ready(&self, model: &ModelId) -> Result<(), ProviderError>;
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResult, ProviderError>;
    async fn cancel(&self, request_id: RequestId) -> Result<(), ProviderError>;
}

#[derive(Clone, Debug)]
pub struct RemoteWorkerProvider {
    endpoint: Url,
    client: reqwest::Client,
    authorization: header::HeaderValue,
    capabilities: Arc<Mutex<ProviderCapabilities>>,
}

impl RemoteWorkerProvider {
    pub fn new(
        endpoint: &str,
        bearer_token: &str,
        allow_insecure_loopback_development: bool,
    ) -> Result<Self, ProviderError> {
        Self::new_with_timeouts(
            endpoint,
            bearer_token,
            allow_insecure_loopback_development,
            Duration::from_secs(5),
            Duration::from_secs(10 * 60),
        )
    }

    /// Constructs a remote client with explicit, bounded user-configurable
    /// timeouts. The credentials remain in the caller and are never retained
    /// outside the HTTP authorization header.
    pub fn new_with_timeouts(
        endpoint: &str,
        bearer_token: &str,
        allow_insecure_loopback_development: bool,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        if !(Duration::from_secs(1)..=Duration::from_secs(60)).contains(&connect_timeout) {
            return Err(ProviderError::Protocol(
                "connect timeout must be between one and sixty seconds".to_owned(),
            ));
        }
        if !(Duration::from_secs(5)..=Duration::from_secs(10 * 60)).contains(&request_timeout) {
            return Err(ProviderError::Protocol(
                "request timeout must be between five seconds and ten minutes".to_owned(),
            ));
        }
        let mut endpoint = Url::parse(endpoint)
            .map_err(|error| ProviderError::InsecureEndpoint(format!("invalid URL: {error}")))?;
        validate_endpoint(&endpoint, allow_insecure_loopback_development)?;
        if !endpoint.path().ends_with('/') {
            let path = format!("{}/", endpoint.path());
            endpoint.set_path(&path);
        }
        if bearer_token.len() < 16 || bearer_token.len() > 512 {
            return Err(ProviderError::Protocol(
                "bearer token must contain 16 to 512 characters".to_owned(),
            ));
        }
        let authorization = header::HeaderValue::from_str(&format!("Bearer {bearer_token}"))
            .map_err(|_| {
                ProviderError::Protocol(
                    "bearer token contains invalid header characters".to_owned(),
                )
            })?;
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .build()
            .map_err(|error| {
                ProviderError::Unavailable(format!("cannot create HTTP client: {error}"))
            })?;
        Ok(Self {
            endpoint,
            client,
            authorization,
            capabilities: Arc::new(Mutex::new(unknown_remote_capabilities())),
        })
    }

    pub async fn refresh_capabilities(&self) -> Result<ProviderCapabilities, ProviderError> {
        let response: CapabilitiesResponseV1 = self.get_json("v1/capabilities").await?;
        let capabilities = capabilities_from_wire(response)?;
        *self.capabilities.lock().map_err(|_| {
            ProviderError::Unavailable("capability state is unavailable".to_owned())
        })? = capabilities.clone();
        Ok(capabilities)
    }

    fn url(&self, suffix: &str) -> Result<Url, ProviderError> {
        self.endpoint
            .join(suffix)
            .map_err(|error| ProviderError::Protocol(format!("cannot build worker URL: {error}")))
    }

    async fn get_json<T: serde::de::DeserializeOwned + VersionedResponse>(
        &self,
        suffix: &str,
    ) -> Result<T, ProviderError> {
        let response = self
            .client
            .get(self.url(suffix)?)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .send()
            .await
            .map_err(network_error)?;
        parse_response(response).await
    }
}

#[async_trait]
impl TranscriptionProvider for RemoteWorkerProvider {
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let health: free_whisper_protocol::HealthResponseV1 = self.get_json("healthz").await?;
        Ok(ProviderHealth {
            healthy: health.status == free_whisper_protocol::HealthStatus::Ready,
            detail: match health.status {
                free_whisper_protocol::HealthStatus::Ready => "worker ready".to_owned(),
                free_whisper_protocol::HealthStatus::Degraded => "worker degraded".to_owned(),
            },
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| unknown_remote_capabilities())
    }

    async fn list_models(&self) -> Result<Vec<AvailableModel>, ProviderError> {
        let response: ModelsResponseV1 = self.get_json("v1/models").await?;
        response
            .models
            .into_iter()
            .map(|model| {
                Ok(AvailableModel {
                    id: ModelId::parse(model.id).map_err(|error| {
                        ProviderError::Protocol(format!("invalid worker model id: {error}"))
                    })?,
                    display_name: model.display_name,
                    installed: model.installed,
                    ready: model.ready,
                })
            })
            .collect()
    }

    async fn ensure_model_ready(&self, model: &ModelId) -> Result<(), ProviderError> {
        let models = self.list_models().await?;
        match models.into_iter().find(|candidate| candidate.id == *model) {
            Some(candidate) if candidate.installed && candidate.ready => Ok(()),
            Some(_) => Err(ProviderError::ModelNotReady(model.as_str().to_owned())),
            None => Err(ProviderError::ModelNotReady(format!(
                "{} is unknown to the worker",
                model.as_str()
            ))),
        }
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResult, ProviderError> {
        let options = options_from_request(&request)?;
        let mut pcm = Vec::with_capacity(request.audio.samples.len().saturating_mul(4));
        for sample in &request.audio.samples {
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        let body = reqwest::multipart::Form::new()
            .text(
                "options",
                serde_json::to_string(&options).map_err(|error| {
                    ProviderError::Protocol(format!("cannot serialize options: {error}"))
                })?,
            )
            .text("audio_encoding", "pcm_f32le_mono_16khz")
            .part(
                "audio",
                reqwest::multipart::Part::bytes(pcm)
                    .file_name("recording.pcm")
                    .mime_str("application/octet-stream")
                    .map_err(|error| {
                        ProviderError::Protocol(format!("cannot encode audio part: {error}"))
                    })?,
            );
        let response = self
            .client
            .post(self.url("v1/transcriptions")?)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .multipart(body)
            .send()
            .await
            .map_err(network_error)?;
        let result: TranscriptionResponseV1 = parse_response(response).await?;
        result_from_wire(result, request.id)
    }

    async fn cancel(&self, request_id: RequestId) -> Result<(), ProviderError> {
        let response = self
            .client
            .post(self.url(&format!(
                "v1/transcriptions/{}/cancel",
                request_id.as_uuid()
            ))?)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .send()
            .await
            .map_err(network_error)?;
        let cancel: free_whisper_protocol::CancelResponseV1 = parse_response(response).await?;
        if !cancel.cancelled || cancel.request_id != request_id.as_uuid() {
            return Err(ProviderError::Protocol(
                "worker returned a mismatched cancellation response".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct LocalSidecarConfig {
    pub worker_executable: PathBuf,
    pub whisper_server_executable: PathBuf,
    pub model_path: PathBuf,
    pub model_id: ModelId,
    pub backend: ExecutionBackend,
    pub startup_timeout: Duration,
}

#[derive(Debug)]
struct StartedSidecar {
    child: Child,
    provider: RemoteWorkerProvider,
}

/// Launches the packaged worker on a randomly selected loopback port. The
/// bearer token is generated in memory and written only through the child stdin.
#[derive(Debug)]
pub struct LocalWhisperCppProvider {
    config: LocalSidecarConfig,
    started: AsyncMutex<Option<StartedSidecar>>,
    start_gate: AsyncMutex<()>,
    capabilities: Mutex<ProviderCapabilities>,
}

impl LocalWhisperCppProvider {
    #[must_use]
    pub fn new(config: LocalSidecarConfig) -> Self {
        Self {
            config,
            started: AsyncMutex::new(None),
            start_gate: AsyncMutex::new(()),
            capabilities: Mutex::new(unknown_local_capabilities()),
        }
    }

    pub async fn shutdown(&self) -> Result<(), ProviderError> {
        let _start_gate = self.start_gate.lock().await;
        let sidecar = { self.started.lock().await.take() };
        if let Some(mut sidecar) = sidecar
            && sidecar
                .child
                .try_wait()
                .map_err(|error| {
                    ProviderError::Unavailable(format!("cannot inspect local worker: {error}"))
                })?
                .is_none()
        {
            sidecar.child.kill().await.map_err(|error| {
                ProviderError::Unavailable(format!("cannot stop local worker: {error}"))
            })?;
        }
        Ok(())
    }

    async fn provider(&self) -> Result<RemoteWorkerProvider, ProviderError> {
        if let Some(provider) = self.existing_provider().await? {
            return Ok(provider);
        }
        let _start_gate = self.start_gate.lock().await;
        if let Some(provider) = self.existing_provider().await? {
            return Ok(provider);
        }
        if !self.config.worker_executable.is_file()
            || !self.config.whisper_server_executable.is_file()
            || !self.config.model_path.is_file()
        {
            return Err(ProviderError::ModelNotReady(
                "local worker executable, whisper.cpp server, or selected model is missing"
                    .to_owned(),
            ));
        }
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut child = Command::new(&self.config.worker_executable)
            .arg("--token-stdin")
            .arg("--bind")
            .arg("127.0.0.1:0")
            .arg("--whisper-server")
            .arg(&self.config.whisper_server_executable)
            .arg("--model")
            .arg(&self.config.model_path)
            .arg("--model-id")
            .arg(self.config.model_id.as_str())
            .arg("--backend")
            .arg(backend_to_wire(&self.config.backend))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                ProviderError::Unavailable(format!("cannot start local worker sidecar: {error}"))
            })?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::Unavailable(
                "local worker stdin is unavailable for token handoff".to_owned(),
            )
        })?;
        stdin
            .write_all(format!("{token}\n").as_bytes())
            .await
            .map_err(|error| {
                ProviderError::Unavailable(format!("cannot send local worker token: {error}"))
            })?;
        stdin.shutdown().await.map_err(|error| {
            ProviderError::Unavailable(format!("cannot close local worker token channel: {error}"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::Unavailable(
                "local worker stdout is unavailable for readiness".to_owned(),
            )
        })?;
        let bound = tokio::time::timeout(self.config.startup_timeout, read_bound_address(stdout))
            .await
            .map_err(|_| {
                ProviderError::Unavailable(
                    "local worker did not report its bound address".to_owned(),
                )
            })??;
        let provider = RemoteWorkerProvider::new(&format!("http://{bound}"), &token, true)?;
        let deadline = tokio::time::Instant::now() + self.config.startup_timeout;
        while tokio::time::Instant::now() < deadline {
            if provider.health_check().await.is_ok() {
                *self.started.lock().await = Some(StartedSidecar {
                    child,
                    provider: provider.clone(),
                });
                return Ok(provider);
            }
            if let Some(status) = child.try_wait().map_err(|error| {
                ProviderError::Unavailable(format!("cannot inspect starting local worker: {error}"))
            })? {
                return Err(ProviderError::Unavailable(format!(
                    "local worker exited during startup with {status}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        child.kill().await.map_err(|error| {
            ProviderError::Unavailable(format!("cannot stop timed-out local worker: {error}"))
        })?;
        Err(ProviderError::Unavailable(
            "local worker did not become ready before its startup timeout".to_owned(),
        ))
    }

    async fn existing_provider(&self) -> Result<Option<RemoteWorkerProvider>, ProviderError> {
        let mut started = self.started.lock().await;
        let Some(sidecar) = started.as_mut() else {
            return Ok(None);
        };
        if sidecar
            .child
            .try_wait()
            .map_err(|error| {
                ProviderError::Unavailable(format!("cannot inspect local worker: {error}"))
            })?
            .is_none()
        {
            return Ok(Some(sidecar.provider.clone()));
        }
        *started = None;
        Ok(None)
    }

    async fn refresh_local_capabilities(
        &self,
        provider: &RemoteWorkerProvider,
    ) -> Result<(), ProviderError> {
        let capabilities = provider.refresh_capabilities().await?;
        *self.capabilities.lock().map_err(|_| {
            ProviderError::Unavailable("local capability state is unavailable".to_owned())
        })? = capabilities;
        Ok(())
    }
}

async fn read_bound_address(
    stdout: tokio::process::ChildStdout,
) -> Result<std::net::SocketAddr, ProviderError> {
    let mut line = String::new();
    let bytes = BufReader::new(stdout)
        .read_line(&mut line)
        .await
        .map_err(|error| {
            ProviderError::Unavailable(format!("cannot read local worker readiness: {error}"))
        })?;
    let address = line
        .strip_prefix("FREE_WHISPER_BOUND ")
        .ok_or_else(|| {
            ProviderError::Protocol("local worker emitted an invalid readiness line".to_owned())
        })?
        .trim()
        .parse::<std::net::SocketAddr>()
        .map_err(|_| {
            ProviderError::Protocol("local worker reported an invalid address".to_owned())
        })?;
    if bytes == 0 || !address.ip().is_loopback() || address.port() == 0 {
        return Err(ProviderError::Protocol(
            "local worker reported a non-loopback or unresolved address".to_owned(),
        ));
    }
    Ok(address)
}

const fn backend_to_wire(value: &ExecutionBackend) -> &'static str {
    match value {
        ExecutionBackend::Cpu => "cpu",
        ExecutionBackend::Vulkan => "vulkan",
        ExecutionBackend::Cuda => "cuda",
        ExecutionBackend::OpenVino => "open_vino",
    }
}

#[async_trait]
impl TranscriptionProvider for LocalWhisperCppProvider {
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let provider = self.provider().await?;
        let health = provider.health_check().await?;
        self.refresh_local_capabilities(&provider).await?;
        Ok(health)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| unknown_local_capabilities())
    }

    async fn list_models(&self) -> Result<Vec<AvailableModel>, ProviderError> {
        let provider = self.provider().await?;
        self.refresh_local_capabilities(&provider).await?;
        provider.list_models().await
    }

    async fn ensure_model_ready(&self, model: &ModelId) -> Result<(), ProviderError> {
        let provider = self.provider().await?;
        self.refresh_local_capabilities(&provider).await?;
        provider.ensure_model_ready(model).await
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResult, ProviderError> {
        let provider = self.provider().await?;
        provider.transcribe(request).await
    }

    async fn cancel(&self, request_id: RequestId) -> Result<(), ProviderError> {
        let provider = self.provider().await?;
        provider.cancel(request_id).await
    }
}

fn validate_endpoint(
    endpoint: &Url,
    allow_insecure_loopback_development: bool,
) -> Result<(), ProviderError> {
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ProviderError::InsecureEndpoint(
            "URL must not contain user info, query or fragment".to_owned(),
        ));
    }
    if endpoint.scheme() == "https" {
        return Ok(());
    }
    if endpoint.scheme() == "http"
        && allow_insecure_loopback_development
        && endpoint_is_loopback(endpoint)
    {
        return Ok(());
    }
    Err(ProviderError::InsecureEndpoint(
        "remote workers require HTTPS; HTTP is permitted only for explicit loopback development"
            .to_owned(),
    ))
}

fn endpoint_is_loopback(endpoint: &Url) -> bool {
    match endpoint.host() {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        _ => false,
    }
}

async fn parse_response<T: serde::de::DeserializeOwned + VersionedResponse>(
    response: reqwest::Response,
) -> Result<T, ProviderError> {
    if response.status().is_success() {
        let parsed = response.json().await.map_err(|error| {
            ProviderError::Protocol(format!("worker returned invalid JSON: {error}"))
        })?;
        validate_api_version(&parsed)?;
        return Ok(parsed);
    }
    let status = response.status();
    let error = response.json::<ApiErrorResponseV1>().await.ok();
    Err(match error {
        Some(error) => {
            validate_api_version(&error)?;
            map_api_error(
                status,
                error.error.code,
                error.error.message,
                error.error.request_id,
            )
        }
        None => ProviderError::Protocol(format!(
            "worker returned HTTP {status} without a v1 error body"
        )),
    })
}

fn validate_api_version(response: &impl VersionedResponse) -> Result<(), ProviderError> {
    if response.api_version() == free_whisper_protocol::API_VERSION {
        Ok(())
    } else {
        Err(ProviderError::Protocol(format!(
            "worker uses unsupported API version {}; expected {}",
            response.api_version(),
            free_whisper_protocol::API_VERSION
        )))
    }
}

fn map_api_error(
    status: StatusCode,
    code: ApiErrorCodeV1,
    message: String,
    request_id: Option<Uuid>,
) -> ProviderError {
    match code {
        ApiErrorCodeV1::Unauthorized => ProviderError::Unauthorized,
        ApiErrorCodeV1::ModelNotReady => ProviderError::ModelNotReady(message),
        ApiErrorCodeV1::Cancelled => request_id.map_or_else(
            || ProviderError::Protocol("worker cancellation error omitted request_id".to_owned()),
            |request_id| ProviderError::Cancelled(RequestId::from_uuid(request_id)),
        ),
        ApiErrorCodeV1::EngineUnavailable | ApiErrorCodeV1::Busy | ApiErrorCodeV1::RateLimited => {
            ProviderError::Unavailable(message)
        }
        ApiErrorCodeV1::EngineProtocol => ProviderError::Protocol(message),
        _ => ProviderError::Rejected(format!("HTTP {status}: {message}")),
    }
}

fn network_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() || error.is_connect() {
        ProviderError::Unavailable("network connection to worker failed or timed out".to_owned())
    } else {
        ProviderError::Unavailable(format!("worker request failed: {error}"))
    }
}

fn options_from_request(
    request: &TranscriptionRequest,
) -> Result<TranscriptionOptionsV1, ProviderError> {
    if request.audio.sample_rate_hz != 16_000 {
        return Err(ProviderError::Protocol(
            "worker v1 requires 16 kHz PCM input".to_owned(),
        ));
    }
    if !request.decoding.temperature.is_finite()
        || !(0.0..=1.0).contains(&request.decoding.temperature)
    {
        return Err(ProviderError::Protocol(
            "temperature must be finite and within 0.0..=1.0".to_owned(),
        ));
    }
    let temperature_milli = (request.decoding.temperature * 1_000.0).round() as u16;
    Ok(TranscriptionOptionsV1 {
        request_id: request.id.as_uuid(),
        language: match &request.language {
            free_whisper_domain::LanguageSelection::Auto => LanguageRequestV1::Auto,
            free_whisper_domain::LanguageSelection::Explicit(value) => {
                LanguageRequestV1::Explicit(value.clone())
            }
        },
        prompt_terms: request.prompt_terms.clone(),
        temperature_milli,
        beam_size: request.decoding.beam_size,
        word_timestamps: request.decoding.word_timestamps,
    })
}

fn capabilities_from_wire(
    value: CapabilitiesResponseV1,
) -> Result<ProviderCapabilities, ProviderError> {
    let gpu = value
        .gpu
        .map(|gpu| {
            Ok(free_whisper_domain::GpuInformation {
                backend: backend_from_wire(gpu.backend),
                name: gpu.name,
                available: gpu.available,
            })
        })
        .transpose()?;
    Ok(ProviderCapabilities {
        local_execution: value.local_execution,
        remote_execution: value.remote_execution,
        streaming: value.streaming,
        vad: value.vad,
        word_timestamps: value.word_timestamps,
        model_switching: value.model_switching,
        language_detection: value.language_detection,
        gpu,
        max_request_bytes: value.max_request_bytes,
        max_audio_duration_ms: value.max_audio_duration_ms,
    })
}

fn unknown_remote_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        local_execution: false,
        remote_execution: true,
        streaming: false,
        vad: false,
        word_timestamps: false,
        model_switching: false,
        language_detection: false,
        gpu: None,
        max_request_bytes: 0,
        max_audio_duration_ms: 0,
    }
}

fn unknown_local_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        local_execution: true,
        remote_execution: false,
        streaming: false,
        vad: false,
        word_timestamps: false,
        model_switching: false,
        language_detection: false,
        gpu: None,
        max_request_bytes: 0,
        max_audio_duration_ms: 0,
    }
}

fn result_from_wire(
    result: TranscriptionResponseV1,
    request_id: RequestId,
) -> Result<TranscriptionResult, ProviderError> {
    if result.request_id != request_id.as_uuid() {
        return Err(ProviderError::Protocol(
            "worker response request id does not match".to_owned(),
        ));
    }
    let model_id = ModelId::parse(result.metadata.model_id).map_err(|error| {
        ProviderError::Protocol(format!("invalid worker result model id: {error}"))
    })?;
    Ok(TranscriptionResult {
        raw_text: result.raw_text.clone(),
        final_text: result.raw_text,
        language_detected: result.language_detected,
        language_confidence_milli: result.language_confidence_milli,
        segments: result
            .segments
            .into_iter()
            .map(|segment| free_whisper_domain::TranscriptSegment {
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                text: segment.text,
            })
            .collect(),
        provider: free_whisper_domain::ProviderMetadata {
            provider_id: result.metadata.provider_id,
            model_id,
            backend: backend_from_wire(result.metadata.backend),
            quantization: result.metadata.quantization,
        },
        durations: free_whisper_domain::ProcessingDurations {
            inference_ms: result.metadata.inference_ms,
            ..Default::default()
        },
        warnings: result.warnings,
        corrections: Vec::new(),
    })
}

const fn backend_from_wire(value: free_whisper_protocol::ExecutionBackendV1) -> ExecutionBackend {
    match value {
        free_whisper_protocol::ExecutionBackendV1::Cpu => ExecutionBackend::Cpu,
        free_whisper_protocol::ExecutionBackendV1::Cuda => ExecutionBackend::Cuda,
        free_whisper_protocol::ExecutionBackendV1::Vulkan => ExecutionBackend::Vulkan,
        free_whisper_protocol::ExecutionBackendV1::OpenVino => ExecutionBackend::OpenVino,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use free_whisper_domain::{DecodingOptions, LanguageSelection, PcmF32Mono};
    use free_whisper_protocol::{
        API_VERSION, ApiErrorCodeV1, ApiErrorResponseV1, ApiErrorV1, ExecutionBackendV1,
        HealthResponseV1, HealthStatus, InferenceMetadataV1, ModelInfoV1, TranscriptSegmentV1,
    };
    use free_whisper_worker::{
        EngineError, EngineRequest, MAX_AUDIO_DURATION_MS, MAX_REQUEST_BYTES, WorkerEngine,
        WorkerState, serve,
    };

    use super::*;

    #[derive(Debug)]
    struct ContractEngine;

    #[async_trait]
    impl WorkerEngine for ContractEngine {
        async fn health_check(&self) -> Result<(), EngineError> {
            Ok(())
        }

        fn capabilities(&self) -> CapabilitiesResponseV1 {
            CapabilitiesResponseV1 {
                api_version: API_VERSION,
                local_execution: true,
                remote_execution: true,
                streaming: false,
                vad: false,
                word_timestamps: true,
                model_switching: false,
                language_detection: true,
                gpu: None,
                max_request_bytes: MAX_REQUEST_BYTES as u64,
                max_audio_duration_ms: MAX_AUDIO_DURATION_MS,
            }
        }

        async fn models(&self) -> Result<ModelsResponseV1, EngineError> {
            Ok(ModelsResponseV1 {
                api_version: API_VERSION,
                models: vec![ModelInfoV1 {
                    id: "test-model".to_owned(),
                    display_name: "Test model".to_owned(),
                    installed: true,
                    ready: true,
                }],
            })
        }

        async fn transcribe(
            &self,
            request: EngineRequest,
        ) -> Result<TranscriptionResponseV1, EngineError> {
            Ok(TranscriptionResponseV1 {
                api_version: API_VERSION,
                request_id: request.options.request_id,
                raw_text: "vertrags-test".to_owned(),
                language_detected: Some("de".to_owned()),
                language_confidence_milli: Some(900),
                segments: vec![TranscriptSegmentV1 {
                    start_ms: 0,
                    end_ms: request.audio.duration_ms,
                    text: "vertrags-test".to_owned(),
                }],
                metadata: InferenceMetadataV1 {
                    provider_id: "contract-engine".to_owned(),
                    model_id: "test-model".to_owned(),
                    backend: ExecutionBackendV1::Cpu,
                    quantization: None,
                    inference_ms: 1,
                },
                warnings: Vec::new(),
            })
        }

        async fn cancel(&self, _request_id: Uuid) -> Result<(), EngineError> {
            Ok(())
        }
    }

    #[test]
    fn endpoint_policy_requires_https_except_explicit_loopback_development() {
        assert!(
            RemoteWorkerProvider::new(
                "https://worker.example.test",
                "x".repeat(16).as_str(),
                false
            )
            .is_ok()
        );
        assert!(matches!(
            RemoteWorkerProvider::new("http://worker.example.test", "x".repeat(16).as_str(), true),
            Err(ProviderError::InsecureEndpoint(_))
        ));
        assert!(
            RemoteWorkerProvider::new("http://127.0.0.1:8999", "x".repeat(16).as_str(), true)
                .is_ok()
        );
    }

    #[test]
    fn remote_timeouts_are_bounded_before_any_network_request() {
        assert!(matches!(
            RemoteWorkerProvider::new_with_timeouts(
                "https://worker.example.test",
                "x".repeat(16).as_str(),
                false,
                Duration::from_millis(999),
                Duration::from_secs(10),
            ),
            Err(ProviderError::Protocol(_))
        ));
        assert!(matches!(
            RemoteWorkerProvider::new_with_timeouts(
                "https://worker.example.test",
                "x".repeat(16).as_str(),
                false,
                Duration::from_secs(5),
                Duration::from_secs(601),
            ),
            Err(ProviderError::Protocol(_))
        ));
    }

    #[test]
    fn unknown_backend_is_rejected_by_the_versioned_wire_contract() {
        assert!(serde_json::from_str::<ExecutionBackendV1>("\"mystery-gpu\"").is_err());
    }

    #[test]
    fn another_api_version_is_a_protocol_error() {
        let error = validate_api_version(&free_whisper_protocol::HealthResponseV1 {
            api_version: API_VERSION + 1,
            status: free_whisper_protocol::HealthStatus::Ready,
            worker_instance_id: Uuid::nil(),
        })
        .expect_err("another API version must not be accepted");

        assert!(matches!(error, ProviderError::Protocol(_)));
    }

    #[test]
    fn inner_engine_protocol_error_is_not_flattened_to_a_rejected_request() {
        let error = map_api_error(
            StatusCode::BAD_GATEWAY,
            ApiErrorCodeV1::EngineProtocol,
            "inner engine response did not match the pinned contract".to_owned(),
            None,
        );

        assert!(matches!(error, ProviderError::Protocol(_)));
    }

    #[tokio::test]
    async fn client_rejects_a_different_version_on_success_and_error_responses() {
        let success = serde_json::to_vec(&HealthResponseV1 {
            api_version: API_VERSION + 1,
            status: HealthStatus::Ready,
            worker_instance_id: Uuid::nil(),
        })
        .expect("success JSON");
        let endpoint = serve_one_raw_response(200, success).await;
        let provider = RemoteWorkerProvider::new(&endpoint, "x".repeat(16).as_str(), true)
            .expect("loopback endpoint");
        assert!(matches!(
            provider.health_check().await,
            Err(ProviderError::Protocol(_))
        ));

        let error = serde_json::to_vec(&ApiErrorResponseV1 {
            api_version: API_VERSION + 1,
            error: ApiErrorV1 {
                code: ApiErrorCodeV1::Unauthorized,
                message: "different API".to_owned(),
                retryable: false,
                request_id: None,
            },
        })
        .expect("error JSON");
        let endpoint = serve_one_raw_response(401, error).await;
        let provider = RemoteWorkerProvider::new(&endpoint, "x".repeat(16).as_str(), true)
            .expect("loopback endpoint");
        assert!(matches!(
            provider.health_check().await,
            Err(ProviderError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn remote_provider_round_trips_the_worker_v1_contract() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(serve(
            listener,
            WorkerState::new(Arc::new(ContractEngine), "contract-token-1234"),
        ));
        let provider =
            RemoteWorkerProvider::new(&format!("http://{address}"), "contract-token-1234", true)
                .expect("loopback provider");
        assert!(provider.health_check().await.expect("health").healthy);
        assert_eq!(
            provider
                .refresh_capabilities()
                .await
                .expect("capabilities")
                .max_request_bytes,
            MAX_REQUEST_BYTES as u64
        );
        assert_eq!(
            provider.capabilities().max_audio_duration_ms,
            MAX_AUDIO_DURATION_MS
        );
        let model = ModelId::parse("test-model").expect("model");
        provider
            .ensure_model_ready(&model)
            .await
            .expect("model ready");
        let request = TranscriptionRequest {
            id: RequestId::new(),
            audio: PcmF32Mono::new(16_000, vec![0.0; 160]).expect("audio"),
            language: LanguageSelection::Explicit("de".to_owned()),
            decoding: DecodingOptions::default(),
            prompt_terms: vec!["OpenAI".to_owned()],
        };
        let result = provider.transcribe(request).await.expect("transcription");
        assert_eq!(result.raw_text, "vertrags-test");
        assert_eq!(result.provider.model_id, model);
        server.abort();
    }

    async fn serve_one_raw_response(status: u16, body: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("connection");
            let mut request = [0_u8; 1_024];
            let _ = stream.read(&mut request).await.expect("request read");
            let reason = if status == 200 { "OK" } else { "Unauthorized" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("headers");
            stream.write_all(&body).await.expect("body");
        });
        format!("http://{address}")
    }
}
