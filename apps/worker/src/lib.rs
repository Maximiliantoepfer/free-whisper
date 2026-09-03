#![forbid(unsafe_code)]

//! Stable v1 worker boundary. The engine is deliberately behind a trait so an
//! inner whisper.cpp process can be restarted on cancellation without exposing
//! its unversioned HTTP interface to desktop clients.

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use free_whisper_protocol::{
    API_VERSION, ApiErrorCodeV1, ApiErrorResponseV1, AudioEncodingV1, CancelResponseV1,
    CapabilitiesResponseV1, ExecutionBackendV1, HealthResponseV1, HealthStatus, ModelsResponseV1,
    TranscriptionOptionsV1, TranscriptionResponseV1,
};
use serde::Deserialize;
use thiserror::Error;
use tokio::{
    process::{Child, Command},
    sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_AUDIO_DURATION_MS: u64 = 10 * 60 * 1_000;
pub const REQUESTS_PER_MINUTE: usize = 20;

#[derive(Clone)]
pub struct WorkerState {
    engine: Arc<dyn WorkerEngine>,
    bearer_token: Arc<str>,
    worker_instance_id: Uuid,
    active_request: Arc<Semaphore>,
    active_cancellations: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
    request_limiter: Arc<Mutex<VecDeque<Instant>>>,
}

impl WorkerState {
    #[must_use]
    pub fn new(engine: Arc<dyn WorkerEngine>, bearer_token: impl Into<String>) -> Self {
        Self {
            engine,
            bearer_token: Arc::from(bearer_token.into()),
            worker_instance_id: Uuid::new_v4(),
            active_request: Arc::new(Semaphore::new(1)),
            active_cancellations: Arc::new(Mutex::new(HashMap::new())),
            request_limiter: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    async fn acquire_request(&self) -> Result<OwnedSemaphorePermit, ApiFailure> {
        self.active_request
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                ApiFailure::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    ApiErrorCodeV1::Busy,
                    "another transcription is already active",
                    true,
                    None,
                )
            })
    }

    fn check_rate_limit(&self) -> Result<(), ApiFailure> {
        let now = Instant::now();
        let mut timestamps = self.request_limiter.lock().map_err(|_| {
            ApiFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCodeV1::Internal,
                "worker rate-limit state is unavailable",
                true,
                None,
            )
        })?;
        while timestamps
            .front()
            .is_some_and(|timestamp| now.duration_since(*timestamp) >= Duration::from_secs(60))
        {
            timestamps.pop_front();
        }
        if timestamps.len() >= REQUESTS_PER_MINUTE {
            return Err(ApiFailure::new(
                StatusCode::TOO_MANY_REQUESTS,
                ApiErrorCodeV1::RateLimited,
                "worker accepts at most 20 transcription requests per minute",
                true,
                None,
            ));
        }
        timestamps.push_back(now);
        Ok(())
    }

    fn register_request(&self, request_id: Uuid) -> Result<CancellationToken, ApiFailure> {
        let token = CancellationToken::new();
        let mut active = self.active_cancellations.lock().map_err(|_| {
            ApiFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCodeV1::Internal,
                "worker cancellation state is unavailable",
                true,
                Some(request_id),
            )
        })?;
        if active.insert(request_id, token.clone()).is_some() {
            return Err(ApiFailure::new(
                StatusCode::CONFLICT,
                ApiErrorCodeV1::Busy,
                "request id is already active",
                false,
                Some(request_id),
            ));
        }
        Ok(token)
    }

    fn unregister_request(&self, request_id: Uuid) {
        if let Ok(mut active) = self.active_cancellations.lock() {
            active.remove(&request_id);
        }
    }
}

pub fn router(state: WorkerState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/models", get(models))
        .route("/v1/transcriptions", post(transcribe))
        .route("/v1/transcriptions/{id}/cancel", post(cancel))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES + 16 * 1024))
        .with_state(state)
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    state: WorkerState,
) -> Result<(), WorkerError> {
    axum::serve(listener, router(state))
        .await
        .map_err(WorkerError::Serve)
}

pub async fn serve_at(address: SocketAddr, state: WorkerState) -> Result<(), WorkerError> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(WorkerError::Bind)?;
    serve(listener, state).await
}

async fn health(
    State(state): State<WorkerState>,
    headers: HeaderMap,
) -> Result<Json<HealthResponseV1>, ApiFailure> {
    authorize(&headers, &state)?;
    let status = if state.engine.health_check().await.is_ok() {
        HealthStatus::Ready
    } else {
        HealthStatus::Degraded
    };
    Ok(Json(HealthResponseV1 {
        api_version: API_VERSION,
        status,
        worker_instance_id: state.worker_instance_id,
    }))
}

async fn capabilities(
    State(state): State<WorkerState>,
    headers: HeaderMap,
) -> Result<Json<CapabilitiesResponseV1>, ApiFailure> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.capabilities()))
}

async fn models(
    State(state): State<WorkerState>,
    headers: HeaderMap,
) -> Result<Json<ModelsResponseV1>, ApiFailure> {
    authorize(&headers, &state)?;
    state
        .engine
        .models()
        .await
        .map(Json)
        .map_err(|error| map_engine_error(error, None))
}

async fn transcribe(
    State(state): State<WorkerState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<TranscriptionResponseV1>, ApiFailure> {
    authorize(&headers, &state)?;
    state.check_rate_limit()?;
    let permit = state.acquire_request().await?;
    let input = parse_multipart(multipart).await?;
    validate_options(&input.options)?;
    let cancellation = state.register_request(input.options.request_id)?;
    let request_id = input.options.request_id;
    let result = state
        .engine
        .transcribe(EngineRequest {
            audio: input.audio,
            options: input.options,
            cancellation: cancellation.clone(),
        })
        .await;
    drop(permit);
    state.unregister_request(request_id);

    if cancellation.is_cancelled() {
        return Err(ApiFailure::new(
            StatusCode::REQUEST_TIMEOUT,
            ApiErrorCodeV1::Cancelled,
            "transcription was cancelled",
            false,
            Some(request_id),
        ));
    }
    result
        .map(Json)
        .map_err(|error| map_engine_error(error, Some(request_id)))
}

async fn cancel(
    State(state): State<WorkerState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<CancelResponseV1>, ApiFailure> {
    authorize(&headers, &state)?;
    let cancellation = state
        .active_cancellations
        .lock()
        .map_err(|_| {
            ApiFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCodeV1::Internal,
                "worker cancellation state is unavailable",
                true,
                Some(id),
            )
        })?
        .get(&id)
        .cloned()
        .ok_or_else(|| {
            ApiFailure::new(
                StatusCode::NOT_FOUND,
                ApiErrorCodeV1::NotFound,
                "no active transcription has this request id",
                false,
                Some(id),
            )
        })?;
    cancellation.cancel();
    state
        .engine
        .cancel(id)
        .await
        .map_err(|error| map_engine_error(error, Some(id)))?;
    Ok(Json(CancelResponseV1 {
        api_version: API_VERSION,
        request_id: id,
        cancelled: true,
    }))
}

fn authorize(headers: &HeaderMap, state: &WorkerState) -> Result<(), ApiFailure> {
    let expected = format!("Bearer {}", state.bearer_token);
    let received = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if received != Some(expected.as_str()) {
        return Err(ApiFailure::new(
            StatusCode::UNAUTHORIZED,
            ApiErrorCodeV1::Unauthorized,
            "a valid bearer token is required",
            false,
            None,
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct MultipartInput {
    options: TranscriptionOptionsV1,
    audio: EngineAudio,
}

async fn parse_multipart(mut multipart: Multipart) -> Result<MultipartInput, ApiFailure> {
    let mut options = None;
    let mut encoding = None;
    let mut audio = None;
    while let Some(field) = multipart.next_field().await.map_err(|error| {
        ApiFailure::new(
            StatusCode::BAD_REQUEST,
            ApiErrorCodeV1::InvalidRequest,
            format!("invalid multipart request: {error}"),
            false,
            None,
        )
    })? {
        let field_name = field.name().unwrap_or_default().to_owned();
        match field_name.as_str() {
            "options" => {
                if options.is_some() {
                    return Err(invalid_request("options was sent more than once"));
                }
                let text = field
                    .text()
                    .await
                    .map_err(|_| invalid_request("options is not UTF-8"))?;
                options = Some(serde_json::from_str(&text).map_err(|error| {
                    ApiFailure::new(
                        StatusCode::BAD_REQUEST,
                        ApiErrorCodeV1::InvalidRequest,
                        format!("invalid options JSON: {error}"),
                        false,
                        None,
                    )
                })?);
            }
            "audio_encoding" => {
                if encoding.is_some() {
                    return Err(invalid_request("audio_encoding was sent more than once"));
                }
                let text = field
                    .text()
                    .await
                    .map_err(|_| invalid_request("audio_encoding is not UTF-8"))?;
                encoding = Some(match text.as_str() {
                    "wav" => AudioEncodingV1::Wav,
                    "pcm_f32le_mono_16khz" => AudioEncodingV1::PcmF32LeMono16Khz,
                    _ => return Err(invalid_request("unsupported audio_encoding")),
                });
            }
            "audio" => {
                if audio.is_some() {
                    return Err(invalid_request("audio was sent more than once"));
                }
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| invalid_request("audio could not be read"))?;
                if bytes.len() > MAX_REQUEST_BYTES {
                    return Err(ApiFailure::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        ApiErrorCodeV1::RequestTooLarge,
                        "audio exceeds the 32 MiB request limit",
                        false,
                        None,
                    ));
                }
                audio = Some(bytes.to_vec());
            }
            _ => {
                return Err(invalid_request(
                    "multipart request contains an unknown field",
                ));
            }
        }
    }
    let options = options.ok_or_else(|| invalid_request("missing options field"))?;
    let encoding = encoding.ok_or_else(|| invalid_request("missing audio_encoding field"))?;
    let bytes = audio.ok_or_else(|| invalid_request("missing audio field"))?;
    validate_audio(encoding, &bytes).map(|duration_ms| MultipartInput {
        options,
        audio: EngineAudio {
            encoding,
            bytes,
            duration_ms,
        },
    })
}

fn validate_options(options: &TranscriptionOptionsV1) -> Result<(), ApiFailure> {
    if options.prompt_terms.len() > 32
        || options
            .prompt_terms
            .iter()
            .any(|term| term.trim().is_empty())
        || options
            .prompt_terms
            .iter()
            .map(|term| term.chars().count().div_ceil(2).max(1))
            .sum::<usize>()
            > 128
        || options.beam_size == 0
        || options.beam_size > 10
        || options.temperature_milli > 1_000
    {
        return Err(invalid_request("options exceed the v1 bounds"));
    }
    Ok(())
}

fn validate_audio(encoding: AudioEncodingV1, bytes: &[u8]) -> Result<u64, ApiFailure> {
    let duration_ms = match encoding {
        AudioEncodingV1::PcmF32LeMono16Khz => validate_pcm_f32(bytes)?,
        AudioEncodingV1::Wav => validate_wav(bytes)?,
    };
    if duration_ms > MAX_AUDIO_DURATION_MS {
        return Err(ApiFailure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ApiErrorCodeV1::AudioTooLong,
            "audio exceeds the ten-minute limit",
            false,
            None,
        ));
    }
    Ok(duration_ms)
}

fn validate_pcm_f32(bytes: &[u8]) -> Result<u64, ApiFailure> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return Err(invalid_request(
            "PCM must contain non-empty f32 little-endian samples",
        ));
    }
    for sample in bytes.chunks_exact(4) {
        let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
        if !value.is_finite() {
            return Err(invalid_request("PCM contains a non-finite sample"));
        }
    }
    Ok(u64::try_from(bytes.len() / 4)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000)
        / 16_000)
}

fn validate_wav(bytes: &[u8]) -> Result<u64, ApiFailure> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid_request("audio is not a RIFF/WAVE file"));
    }
    let mut offset: usize = 12;
    let mut format = None;
    let mut data = None;
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]);
        let chunk_start = offset + 8;
        let chunk_end = chunk_start
            .checked_add(
                usize::try_from(chunk_size).map_err(|_| invalid_request("WAV chunk too large"))?,
            )
            .ok_or_else(|| invalid_request("WAV chunk overflows"))?;
        if chunk_end > bytes.len() {
            return Err(invalid_request("WAV chunk is truncated"));
        }
        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err(invalid_request("WAV fmt chunk is too short"));
            }
            format = Some((
                u16::from_le_bytes([bytes[chunk_start], bytes[chunk_start + 1]]),
                u16::from_le_bytes([bytes[chunk_start + 2], bytes[chunk_start + 3]]),
                u32::from_le_bytes([
                    bytes[chunk_start + 4],
                    bytes[chunk_start + 5],
                    bytes[chunk_start + 6],
                    bytes[chunk_start + 7],
                ]),
                u16::from_le_bytes([bytes[chunk_start + 14], bytes[chunk_start + 15]]),
            ));
        } else if chunk_id == b"data" && data.is_none() {
            data = Some(&bytes[chunk_start..chunk_end]);
        }
        offset = chunk_end + usize::from((chunk_size % 2) as u8);
    }
    let (format_code, channels, sample_rate, bits_per_sample) =
        format.ok_or_else(|| invalid_request("WAV is missing fmt chunk"))?;
    let data = data.ok_or_else(|| invalid_request("WAV is missing data chunk"))?;
    if channels != 1
        || sample_rate != 16_000
        || !matches!((format_code, bits_per_sample), (1, 16) | (3, 32))
    {
        return Err(invalid_request(
            "WAV must be mono 16 kHz PCM16 or IEEE-float32",
        ));
    }
    let bytes_per_sample = usize::from(bits_per_sample / 8);
    if data.is_empty() || !data.len().is_multiple_of(bytes_per_sample) {
        return Err(invalid_request("WAV data is incomplete"));
    }
    if format_code == 3 {
        for sample in data.chunks_exact(4) {
            let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
            if !value.is_finite() {
                return Err(invalid_request("WAV contains a non-finite float sample"));
            }
        }
    }
    Ok(u64::try_from(data.len() / bytes_per_sample)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000)
        / 16_000)
}

fn invalid_request(message: &'static str) -> ApiFailure {
    ApiFailure::new(
        StatusCode::BAD_REQUEST,
        ApiErrorCodeV1::InvalidRequest,
        message,
        false,
        None,
    )
}

fn map_engine_error(error: EngineError, request_id: Option<Uuid>) -> ApiFailure {
    match error {
        EngineError::ModelNotReady(message) => ApiFailure::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ApiErrorCodeV1::ModelNotReady,
            &message,
            false,
            request_id,
        ),
        EngineError::Cancelled => ApiFailure::new(
            StatusCode::REQUEST_TIMEOUT,
            ApiErrorCodeV1::Cancelled,
            "transcription was cancelled",
            false,
            request_id,
        ),
        EngineError::Unavailable(message) => ApiFailure::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ApiErrorCodeV1::EngineUnavailable,
            &message,
            true,
            request_id,
        ),
        EngineError::Protocol(message) => ApiFailure::new(
            StatusCode::BAD_GATEWAY,
            ApiErrorCodeV1::EngineProtocol,
            &message,
            false,
            request_id,
        ),
        EngineError::Rejected(message) => ApiFailure::new(
            StatusCode::BAD_GATEWAY,
            ApiErrorCodeV1::Internal,
            &message,
            false,
            request_id,
        ),
    }
}

#[derive(Debug)]
pub struct ApiFailure {
    status: StatusCode,
    code: ApiErrorCodeV1,
    message: String,
    retryable: bool,
    request_id: Option<Uuid>,
}

impl ApiFailure {
    fn new(
        status: StatusCode,
        code: ApiErrorCodeV1,
        message: impl Into<String>,
        retryable: bool,
        request_id: Option<Uuid>,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retryable,
            request_id,
        }
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorResponseV1 {
                api_version: API_VERSION,
                error: free_whisper_protocol::ApiErrorV1 {
                    code: self.code,
                    message: self.message,
                    retryable: self.retryable,
                    request_id: self.request_id,
                },
            }),
        )
            .into_response()
    }
}

#[derive(Clone, Debug)]
pub struct EngineAudio {
    pub encoding: AudioEncodingV1,
    pub bytes: Vec<u8>,
    pub duration_ms: u64,
}

#[derive(Clone, Debug)]
pub struct EngineRequest {
    pub audio: EngineAudio,
    pub options: TranscriptionOptionsV1,
    pub cancellation: CancellationToken,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EngineError {
    #[error("model is not ready: {0}")]
    ModelNotReady(String),
    #[error("engine is unavailable: {0}")]
    Unavailable(String),
    #[error("engine rejected request: {0}")]
    Rejected(String),
    #[error("engine returned an unsupported protocol response: {0}")]
    Protocol(String),
    #[error("engine cancelled the request")]
    Cancelled,
}

#[async_trait]
pub trait WorkerEngine: Send + Sync {
    async fn health_check(&self) -> Result<(), EngineError>;
    fn capabilities(&self) -> CapabilitiesResponseV1;
    async fn models(&self) -> Result<ModelsResponseV1, EngineError>;
    async fn transcribe(
        &self,
        request: EngineRequest,
    ) -> Result<TranscriptionResponseV1, EngineError>;
    async fn cancel(&self, request_id: Uuid) -> Result<(), EngineError>;
}

/// Configuration for a pinned whisper.cpp `whisper-server` process. The worker
/// exposes the public v1 protocol; this inner process is always loopback-only.
#[derive(Clone, Debug)]
pub struct WhisperCppEngineConfig {
    pub executable: PathBuf,
    pub model_path: PathBuf,
    pub model_id: String,
    pub backend: ExecutionBackendV1,
    pub startup_timeout: Duration,
}

#[derive(Debug)]
struct InnerProcess {
    address: SocketAddr,
    child: Child,
}

/// Process-isolated whisper.cpp adapter. It never performs an implicit backend
/// fallback: the configured `backend` is only reported, never substituted.
#[derive(Debug)]
pub struct WhisperCppEngine {
    config: WhisperCppEngineConfig,
    client: reqwest::Client,
    process: AsyncMutex<Option<InnerProcess>>,
    start_gate: AsyncMutex<()>,
}

impl WhisperCppEngine {
    pub fn new(config: WhisperCppEngineConfig) -> Result<Self, EngineError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10 * 60))
            .build()
            .map_err(|error| {
                EngineError::Unavailable(format!("cannot create inner HTTP client: {error}"))
            })?;
        Ok(Self {
            config,
            client,
            process: AsyncMutex::new(None),
            start_gate: AsyncMutex::new(()),
        })
    }

    async fn ensure_running(&self) -> Result<SocketAddr, EngineError> {
        if !self.config.executable.is_file() {
            return Err(EngineError::ModelNotReady(
                "the configured whisper.cpp server executable does not exist".to_owned(),
            ));
        }
        if !self.config.model_path.is_file() {
            return Err(EngineError::ModelNotReady(
                "the configured Whisper model does not exist".to_owned(),
            ));
        }

        if let Some(address) = self.active_address().await? {
            if self.inner_ready(address).await {
                return Ok(address);
            }
            self.stop_at(address).await?;
        }
        let _start_gate = self.start_gate.lock().await;
        if let Some(address) = self.active_address().await? {
            if self.inner_ready(address).await {
                return Ok(address);
            }
            self.stop_at(address).await?;
        }
        let inner = self.start_inner().await?;
        let address = inner.address;
        *self.process.lock().await = Some(inner);
        Ok(address)
    }

    async fn active_address(&self) -> Result<Option<SocketAddr>, EngineError> {
        let mut process = self.process.lock().await;
        let Some(inner) = process.as_mut() else {
            return Ok(None);
        };
        if inner
            .child
            .try_wait()
            .map_err(|error| {
                EngineError::Unavailable(format!("cannot inspect inner engine: {error}"))
            })?
            .is_none()
        {
            return Ok(Some(inner.address));
        }
        *process = None;
        Ok(None)
    }

    async fn stop_at(&self, address: SocketAddr) -> Result<(), EngineError> {
        let inner = {
            let mut process = self.process.lock().await;
            process
                .as_ref()
                .is_some_and(|candidate| candidate.address == address)
                .then(|| process.take())
                .flatten()
        };
        if let Some(inner) = inner {
            stop_child(inner.child).await?;
        }
        Ok(())
    }

    async fn start_inner(&self) -> Result<InnerProcess, EngineError> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|error| {
            EngineError::Unavailable(format!("cannot allocate inner loopback port: {error}"))
        })?;
        let address = listener.local_addr().map_err(|error| {
            EngineError::Unavailable(format!("cannot inspect inner loopback port: {error}"))
        })?;
        drop(listener);
        let child = Command::new(&self.config.executable)
            .arg("--model")
            .arg(&self.config.model_path)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(address.port().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                EngineError::Unavailable(format!("cannot start whisper.cpp server: {error}"))
            })?;
        let mut inner = InnerProcess { address, child };
        let deadline = Instant::now() + self.config.startup_timeout;
        while Instant::now() < deadline {
            if self.inner_ready(address).await {
                return Ok(inner);
            }
            if let Some(status) = inner.child.try_wait().map_err(|error| {
                EngineError::Unavailable(format!("cannot inspect starting engine: {error}"))
            })? {
                return Err(EngineError::Unavailable(format!(
                    "whisper.cpp server exited during startup with {status}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        stop_child(inner.child).await?;
        Err(EngineError::Unavailable(
            "whisper.cpp server did not become ready before its startup timeout".to_owned(),
        ))
    }

    async fn inner_ready(&self, address: SocketAddr) -> bool {
        self.client
            .get(format!("http://{address}/health"))
            .timeout(Duration::from_millis(250))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    async fn stop_and_restart(&self) -> Result<(), EngineError> {
        let _start_gate = self.start_gate.lock().await;
        let existing = { self.process.lock().await.take() };
        if let Some(inner) = existing {
            stop_child(inner.child).await?;
        }
        if !self.config.executable.is_file() || !self.config.model_path.is_file() {
            return Err(EngineError::ModelNotReady(
                "the configured whisper.cpp server or selected model does not exist".to_owned(),
            ));
        }
        let inner = self.start_inner().await?;
        *self.process.lock().await = Some(inner);
        Ok(())
    }
}

#[async_trait]
impl WorkerEngine for WhisperCppEngine {
    async fn health_check(&self) -> Result<(), EngineError> {
        self.ensure_running().await.map(|_| ())
    }

    fn capabilities(&self) -> CapabilitiesResponseV1 {
        CapabilitiesResponseV1 {
            api_version: API_VERSION,
            local_execution: true,
            remote_execution: true,
            streaming: false,
            vad: false,
            // verbose_json gives us segments, not token-level word timestamps.
            // Do not advertise a finer capability than this adapter can prove.
            word_timestamps: false,
            model_switching: false,
            // The server may return a detected language, but does not promise it
            // in its unversioned contract; expose it opportunistically only.
            language_detection: false,
            gpu: (self.config.backend != ExecutionBackendV1::Cpu).then(|| {
                free_whisper_protocol::GpuInfoV1 {
                    backend: self.config.backend,
                    name: "configured worker backend".to_owned(),
                    available: true,
                }
            }),
            max_request_bytes: MAX_REQUEST_BYTES as u64,
            max_audio_duration_ms: MAX_AUDIO_DURATION_MS,
        }
    }

    async fn models(&self) -> Result<ModelsResponseV1, EngineError> {
        let installed = self.config.model_path.is_file();
        let ready = if installed {
            self.ensure_running().await?;
            true
        } else {
            false
        };
        Ok(ModelsResponseV1 {
            api_version: API_VERSION,
            models: vec![free_whisper_protocol::ModelInfoV1 {
                id: self.config.model_id.clone(),
                display_name: self.config.model_id.clone(),
                installed,
                ready,
            }],
        })
    }

    async fn transcribe(
        &self,
        request: EngineRequest,
    ) -> Result<TranscriptionResponseV1, EngineError> {
        if request.cancellation.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let address = self.ensure_running().await?;
        let (audio_bytes, audio_file_name) = match request.audio.encoding {
            AudioEncodingV1::Wav => (request.audio.bytes, "audio.wav"),
            AudioEncodingV1::PcmF32LeMono16Khz => {
                (pcm_f32le_to_wav(&request.audio.bytes)?, "audio.wav")
            }
        };
        let mut form = reqwest::multipart::Form::new()
            .text(
                "temperature",
                format!("{}", request.options.temperature_milli as f32 / 1_000.0),
            )
            .text("beam_size", request.options.beam_size.to_string())
            .text(
                "word_timestamps",
                request.options.word_timestamps.to_string(),
            )
            .text("response_format", "verbose_json")
            .text("prompt", request.options.prompt_terms.join(", "));
        if let free_whisper_protocol::LanguageRequestV1::Explicit(language) =
            &request.options.language
        {
            form = form.text("language", language.clone());
        }
        form = form.part(
            "file",
            reqwest::multipart::Part::bytes(audio_bytes)
                .file_name(audio_file_name)
                .mime_str("audio/wav")
                .map_err(|error| {
                    EngineError::Rejected(format!("cannot form inner audio request: {error}"))
                })?,
        );
        let inference_started = Instant::now();
        let response = self
            .client
            .post(format!("http://{address}/inference"))
            .multipart(form)
            .send()
            .await
            .map_err(|error| {
                EngineError::Unavailable(format!(
                    "whisper.cpp inference connection failed: {error}"
                ))
            })?;
        if !response.status().is_success() {
            return Err(EngineError::Rejected(format!(
                "whisper.cpp inference returned HTTP {}",
                response.status()
            )));
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if !content_type
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
        {
            return Err(EngineError::Protocol(
                "whisper.cpp inference response is not JSON".to_owned(),
            ));
        }
        // Do not use `Response::json` here: an error from it can contain a
        // response excerpt, and the inner response itself may contain a
        // transcript. We deliberately report only the contract failure.
        let body = response.bytes().await.map_err(|_| {
            EngineError::Unavailable("cannot read whisper.cpp inference response".to_owned())
        })?;
        let response: WhisperCppInferenceResponse =
            serde_json::from_slice(&body).map_err(|_| {
                EngineError::Protocol(
                "whisper.cpp inference response does not match the pinned verbose JSON contract"
                    .to_owned(),
            )
            })?;
        let raw_text = response.text;
        let language_detected = response.detected_language.or(response.language);
        let language_confidence_milli = response
            .detected_language_probability
            .map(language_probability_to_milli)
            .transpose()?;
        let segments = response
            .segments
            .into_iter()
            .map(WhisperCppSegment::into_protocol)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TranscriptionResponseV1 {
            api_version: API_VERSION,
            request_id: request.options.request_id,
            raw_text,
            language_detected,
            language_confidence_milli,
            segments,
            metadata: free_whisper_protocol::InferenceMetadataV1 {
                provider_id: "whisper.cpp".to_owned(),
                model_id: self.config.model_id.clone(),
                backend: self.config.backend,
                quantization: None,
                inference_ms: u64::try_from(inference_started.elapsed().as_millis())
                    .unwrap_or(u64::MAX),
            },
            warnings: Vec::new(),
        })
    }

    async fn cancel(&self, _request_id: Uuid) -> Result<(), EngineError> {
        self.stop_and_restart().await
    }
}

fn pcm_f32le_to_wav(pcm: &[u8]) -> Result<Vec<u8>, EngineError> {
    if pcm.is_empty() || !pcm.len().is_multiple_of(4) {
        return Err(EngineError::Rejected(
            "cannot convert malformed PCM to WAV for whisper.cpp".to_owned(),
        ));
    }
    let data_size = u32::try_from(pcm.len())
        .map_err(|_| EngineError::Rejected("PCM is too large for a WAV container".to_owned()))?;
    let riff_size = data_size
        .checked_add(36)
        .ok_or_else(|| EngineError::Rejected("PCM is too large for a WAV container".to_owned()))?;
    let mut wav = Vec::with_capacity(pcm.len() + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&3_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&64_000_u32.to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&32_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    Ok(wav)
}

#[derive(Debug, Deserialize)]
struct WhisperCppInferenceResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    detected_language: Option<String>,
    #[serde(default)]
    detected_language_probability: Option<f64>,
    #[serde(default)]
    segments: Vec<WhisperCppSegment>,
}

#[derive(Debug, Deserialize)]
struct WhisperCppSegment {
    text: String,
    start: f64,
    end: f64,
}

impl WhisperCppSegment {
    fn into_protocol(self) -> Result<free_whisper_protocol::TranscriptSegmentV1, EngineError> {
        let start = seconds_to_millis(self.start, "start")?;
        let end = seconds_to_millis(self.end, "end")?;
        if end < start {
            return Err(EngineError::Protocol(
                "whisper.cpp returned a segment ending before it starts".to_owned(),
            ));
        }
        Ok(free_whisper_protocol::TranscriptSegmentV1 {
            start_ms: start,
            end_ms: end,
            text: self.text,
        })
    }
}

fn seconds_to_millis(seconds: f64, field: &'static str) -> Result<u64, EngineError> {
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return Err(EngineError::Protocol(format!(
            "whisper.cpp returned an invalid segment {field} timestamp"
        )));
    }
    let milliseconds = seconds * 1_000.0;
    if milliseconds > u64::MAX as f64 {
        return Err(EngineError::Protocol(format!(
            "whisper.cpp segment {field} timestamp is too large"
        )));
    }
    Ok(milliseconds.round() as u64)
}

fn language_probability_to_milli(probability: f64) -> Result<u16, EngineError> {
    if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
        return Err(EngineError::Protocol(
            "whisper.cpp returned an invalid detected-language probability".to_owned(),
        ));
    }
    Ok((probability * 1_000.0).round() as u16)
}

async fn stop_child(mut child: Child) -> Result<(), EngineError> {
    if child
        .try_wait()
        .map_err(|error| EngineError::Unavailable(format!("cannot inspect inner engine: {error}")))?
        .is_none()
    {
        child.kill().await.map_err(|error| {
            EngineError::Unavailable(format!("cannot stop inner engine: {error}"))
        })?;
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("cannot bind worker listener: {0}")]
    Bind(std::io::Error),
    #[error("worker server failed: {0}")]
    Serve(std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::{body::Body, http::Request};
    use free_whisper_protocol::{
        ExecutionBackendV1, InferenceMetadataV1, ModelInfoV1, TranscriptSegmentV1,
    };
    use tower::ServiceExt;

    use super::*;

    #[derive(Debug)]
    struct FakeEngine {
        cancelled: AtomicBool,
    }

    #[async_trait]
    impl WorkerEngine for FakeEngine {
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
                    id: "test".to_owned(),
                    display_name: "Test".to_owned(),
                    installed: true,
                    ready: true,
                }],
            })
        }

        async fn transcribe(
            &self,
            request: EngineRequest,
        ) -> Result<TranscriptionResponseV1, EngineError> {
            if request.cancellation.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            Ok(TranscriptionResponseV1 {
                api_version: API_VERSION,
                request_id: request.options.request_id,
                raw_text: "Test".to_owned(),
                language_detected: Some("de".to_owned()),
                language_confidence_milli: Some(999),
                segments: vec![TranscriptSegmentV1 {
                    start_ms: 0,
                    end_ms: request.audio.duration_ms,
                    text: "Test".to_owned(),
                }],
                metadata: InferenceMetadataV1 {
                    provider_id: "fake".to_owned(),
                    model_id: "test".to_owned(),
                    backend: ExecutionBackendV1::Cpu,
                    quantization: None,
                    inference_ms: 1,
                },
                warnings: Vec::new(),
            })
        }

        async fn cancel(&self, _request_id: Uuid) -> Result<(), EngineError> {
            self.cancelled.store(true, Ordering::Release);
            Ok(())
        }
    }

    fn app() -> Router {
        router(WorkerState::new(
            Arc::new(FakeEngine {
                cancelled: AtomicBool::new(false),
            }),
            "test-token",
        ))
    }

    #[tokio::test]
    async fn health_requires_authentication_and_advertises_v1() {
        let unauthorized = app()
            .oneshot(
                Request::get("/healthz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = app()
            .oneshot(
                Request::get("/healthz")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let health: HealthResponseV1 = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(health.api_version, API_VERSION);
        assert_eq!(health.status, HealthStatus::Ready);
    }

    #[test]
    fn audio_validation_rejects_non_finite_and_too_long_pcm() {
        assert!(validate_pcm_f32(&f32::NAN.to_le_bytes()).is_err());
        let too_long = vec![0_u8; (16_000 * 4 * 601) as usize];
        assert!(matches!(
            validate_audio(AudioEncodingV1::PcmF32LeMono16Khz, &too_long),
            Err(ApiFailure {
                code: ApiErrorCodeV1::AudioTooLong,
                ..
            })
        ));
    }

    #[test]
    fn pcm_is_wrapped_as_a_valid_float_wav_for_whisper_cpp() {
        let wav = pcm_f32le_to_wav(&[0_u8; 16]).expect("WAV");
        assert_eq!(validate_wav(&wav).expect("valid WAV"), 0);
    }

    #[test]
    fn pinned_verbose_json_fixture_preserves_language_confidence_and_segments() {
        let response: WhisperCppInferenceResponse = serde_json::from_str(include_str!(
            "../tests/fixtures/whisper-cpp-v1.8.6-verbose.json"
        ))
        .expect("pinned whisper.cpp v1.8.6 verbose JSON fixture");

        assert_eq!(
            response.text,
            "fixture transcript that must never be included in an error"
        );
        assert_eq!(response.detected_language.as_deref(), Some("german"));
        assert_eq!(
            language_probability_to_milli(
                response
                    .detected_language_probability
                    .expect("fixture language probability")
            )
            .expect("valid probability"),
            998
        );
        let segment = response
            .segments
            .into_iter()
            .next()
            .expect("fixture segment")
            .into_protocol()
            .expect("valid segment");
        assert_eq!(segment.start_ms, 230);
        assert_eq!(segment.end_ms, 1_250);
    }

    #[test]
    fn malformed_verbose_segment_is_a_private_protocol_error() {
        let error = WhisperCppSegment {
            text: "private transcript text".to_owned(),
            start: 2.0,
            end: 1.0,
        }
        .into_protocol()
        .expect_err("reverse timestamps are rejected");

        assert!(matches!(error, EngineError::Protocol(_)));
        assert!(!error.to_string().contains("private transcript text"));
    }

    #[test]
    fn invalid_timestamps_and_language_probability_are_protocol_errors() {
        assert!(matches!(
            seconds_to_millis(-0.01, "start"),
            Err(EngineError::Protocol(_))
        ));
        assert!(matches!(
            seconds_to_millis(f64::NAN, "start"),
            Err(EngineError::Protocol(_))
        ));
        assert!(matches!(
            language_probability_to_milli(1.001),
            Err(EngineError::Protocol(_))
        ));
    }
}
