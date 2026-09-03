#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use free_whisper_application::{
    CancelledCapture, CapturePhase, PreparationCommit, RecordingCoordinator,
    RecordingCoordinatorError,
};
use free_whisper_audio::{
    AudioError, AudioRecorder, InputDevice, RecordingHandle, RecordingSettings, RecordingStatus,
};
use free_whisper_domain::{
    AppState, DecodingOptions, ExecutionBackend, InjectionOutcome, JobId, JobStateMachine,
    LanguageSelection, LexiconEntryId, LexiconProfileId, ModelId, ProcessingDurations,
    ProviderProfileId, RequestId, TargetWindowSnapshot, TranscriptCorrectionId, TranscriptId,
    TranscriptionRequest,
};
use free_whisper_lexicon::{
    CorrectionRule, LexiconEntry, LexiconEntryDraft, LexiconProfile, LexiconScope, LexiconVariant,
    LexiconVariantDraft, MatchMode, apply_corrections, select_prompt_terms,
};
use free_whisper_model_manager::{
    DeleteConfirmation, ManifestTrustScope, ManifestVerifier, ModelDownloadProgress, ModelManager,
    ModelManagerError,
};
use free_whisper_platform_windows::{
    ClipboardRestoreOutcome, ClipboardWriter, GlobalHotkey, HotkeyBinding, HotkeyEvent,
    PasteRefusalOrError, SecretStore, SecretStoreError, WindowsClipboard, WindowsCredentialStore,
    WindowsDesktop,
};
use free_whisper_providers::{
    AvailableModel, LocalSidecarConfig, LocalWhisperCppProvider, ProviderError,
    RemoteWorkerProvider, TranscriptionProvider,
};
use free_whisper_storage::{
    Database, InstalledModelRecord, LexiconImportReport, ProviderProfileRecord,
    ProviderProfileSettings, StoredCorrection, StoredTranscript, parse_lexicon_csv,
    parse_lexicon_json,
};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, State, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use time::OffsetDateTime;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const EMBEDDED_MANIFEST: &[u8] = include_bytes!("../../../../resources/models/manifest.json");
const EMBEDDED_SIGNATURE: &str = include_str!("../../../../resources/models/manifest.sig");
const EMBEDDED_PUBLIC_KEY: &str = include_str!("../../../../resources/models/manifest.public-key");
const LOCAL_PROVIDER_ID: &str = "local-whisper-cpp";
const RECORDING_SETTINGS_KEY: &str = "recording.preferences.v1";
const WINDOWS_INTEGRATION_SETTINGS_KEY: &str = "windows.integration.preferences.v1";
const ACTIVE_LEXICON_PROFILE_SETTINGS_KEY: &str = "lexicon.active_profile.v1";
const ACTIVE_REMOTE_PROVIDER_SETTINGS_KEY: &str = "remote.active_provider.v1";
const REMOTE_PROVIDER_ID: &str = "remote-worker";
const CREDENTIAL_REFERENCE_PREFIX: &str = "free-whisper/provider/";

struct DesktopState {
    model_root: PathBuf,
    database: Mutex<Database>,
    downloads: Mutex<BTreeMap<String, CancellationToken>>,
    recorder: AudioRecorder,
    windows: WindowsDesktop,
    credential_store: WindowsCredentialStore,
    hotkey: Mutex<Option<GlobalHotkey>>,
    hotkey_error: Mutex<Option<String>>,
    coordinator: Mutex<RecordingCoordinator>,
    recording: Mutex<Option<ActiveRecording>>,
    processing: Mutex<Option<ProcessingJob>>,
    queued: Mutex<Option<QueuedJob>>,
    processing_available: Notify,
    state_sequence: AtomicU64,
}

struct ActiveRecording {
    job_id: JobId,
    machine: JobStateMachine,
    language: LanguageSelection,
    capture: RecordingHandle,
    provider: Arc<RecordingProvider>,
    model_load_ms: u64,
    target_window: Option<TargetWindowSnapshot>,
}

#[derive(Clone)]
struct ProcessingJob {
    job_id: JobId,
    request_id: RequestId,
    provider: Arc<RecordingProvider>,
}

/// The coordinator owns the only process lifecycle. Remote execution uses the
/// same trait contract but has no local child to stop; this prevents a remote
/// profile from accidentally receiving local-sidecar cleanup semantics.
#[derive(Clone, Debug)]
enum RecordingProvider {
    Local(Arc<LocalWhisperCppProvider>),
    Remote(Arc<RemoteWorkerProvider>),
}

impl RecordingProvider {
    async fn ensure_model_ready(&self, model: &ModelId) -> Result<(), ProviderError> {
        match self {
            Self::Local(provider) => provider.ensure_model_ready(model).await,
            Self::Remote(provider) => provider.ensure_model_ready(model).await,
        }
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<free_whisper_domain::TranscriptionResult, ProviderError> {
        match self {
            Self::Local(provider) => provider.transcribe(request).await,
            Self::Remote(provider) => provider.transcribe(request).await,
        }
    }

    async fn cancel(&self, request_id: RequestId) -> Result<(), ProviderError> {
        match self {
            Self::Local(provider) => provider.cancel(request_id).await,
            Self::Remote(provider) => provider.cancel(request_id).await,
        }
    }

    async fn shutdown(&self) -> Result<(), ProviderError> {
        match self {
            Self::Local(provider) => provider.shutdown().await,
            // A remote worker is owned by its operator. Cancellation is an
            // explicit protocol operation, never a process-management action.
            Self::Remote(_) => Ok(()),
        }
    }
}

struct QueuedJob {
    job_id: JobId,
    cancellation: CancellationToken,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelCatalogItem {
    id: String,
    display_name: String,
    preset: String,
    size_bytes: u64,
    backend: String,
    quantization: String,
    estimated_ram_bytes: u64,
    license: String,
    license_url: String,
    source_url: String,
    installed: bool,
    active: bool,
    validated: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelDownloadEvent {
    api_version: u16,
    model_id: String,
    downloaded_bytes: u64,
    total_bytes: u64,
    bytes_per_second: u64,
    estimated_remaining_seconds: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppStateEvent {
    api_version: u16,
    sequence: u64,
    job_id: String,
    state: AppState,
    detail: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSnapshot {
    app_version: String,
    onboarding: OnboardingState,
    manifest_available: bool,
    manifest_error: Option<String>,
    active_model: Option<InstalledModelView>,
    active_provider: Option<ActiveProviderView>,
    input_devices: Vec<InputDevice>,
    microphone_error: Option<String>,
    recording_settings: RecordingSettings,
    recording_settings_error: Option<String>,
    windows_integration: WindowsIntegrationSettings,
    windows_integration_error: Option<String>,
    hotkey_error: Option<String>,
    capture_phase: CapturePhaseView,
    active_job_id: Option<String>,
    recording: Option<RecordingStatus>,
    processing: bool,
    queue_depth: u8,
    active_lexicon_profile_id: Option<String>,
    last_transcript: Option<TranscriptView>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum CapturePhaseView {
    Idle,
    Preparing,
    Recording,
    Finalizing,
}

impl From<CapturePhase> for CapturePhaseView {
    fn from(value: CapturePhase) -> Self {
        match value {
            CapturePhase::Idle => Self::Idle,
            CapturePhase::Preparing { .. } => Self::Preparing,
            CapturePhase::Recording { .. } => Self::Recording,
            CapturePhase::Finalizing { .. } => Self::Finalizing,
        }
    }
}

/// The backend owns the workspace gate. The UI may render a different shell
/// for setup, but it must not infer readiness from loosely related fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OnboardingState {
    SetupRequired,
    Ready,
}

const fn onboarding_state(has_active_provider: bool) -> OnboardingState {
    if has_active_provider {
        OnboardingState::Ready
    } else {
        OnboardingState::SetupRequired
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstalledModelView {
    id: String,
    display_name: String,
    backend: String,
    quantization: String,
    size_bytes: u64,
    validated: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActiveProviderView {
    kind: &'static str,
    display_name: String,
    model_id: String,
    backend: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptView {
    id: String,
    created_at: String,
    raw_text: String,
    final_text: String,
    language_requested: Option<String>,
    language_detected: Option<String>,
    language_confidence_milli: Option<u16>,
    provider_id: String,
    model_id: String,
    audio_duration_ms: u64,
    processing_ms: u64,
    queue_ms: u64,
    model_load_ms: u64,
    inference_ms: u64,
    correction_ms: u64,
    warnings: Vec<String>,
    corrections: Vec<CorrectionView>,
    injection_outcome: InjectionOutcome,
    can_paste_to_original: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CorrectionView {
    id: String,
    original: String,
    replacement: String,
    start_char: u32,
    end_char: u32,
    reverted: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRecordingInput {
    settings: RecordingSettings,
    /// `None` is language auto-detection. Explicit language values are copied
    /// into each job and cannot change during inference.
    language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowsIntegrationSettings {
    hotkey: HotkeyBinding,
    auto_paste: bool,
    restore_clipboard_after_paste: bool,
}

impl Default for WindowsIntegrationSettings {
    fn default() -> Self {
        Self {
            hotkey: HotkeyBinding::default(),
            auto_paste: false,
            restore_clipboard_after_paste: true,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlatformErrorEvent {
    api_version: u16,
    area: &'static str,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LexiconWorkspaceView {
    active_profile_id: Option<String>,
    profiles: Vec<LexiconProfileView>,
    entries: Vec<LexiconEntryView>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LexiconProfileView {
    id: String,
    name: String,
    enabled: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LexiconEntryView {
    id: String,
    canonical_text: String,
    language: Option<String>,
    category: Option<String>,
    priority: u8,
    enabled: bool,
    profile_id: Option<String>,
    variants: Vec<LexiconVariantView>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LexiconVariantView {
    id: String,
    variant_text: String,
    match_mode: MatchMode,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveLexiconProfileInput {
    id: Option<String>,
    name: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveLexiconEntryInput {
    canonical_text: String,
    language: Option<String>,
    category: Option<String>,
    priority: u8,
    enabled: bool,
    profile_id: Option<String>,
    variants: Vec<SaveLexiconVariantInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveLexiconVariantInput {
    variant_text: String,
    match_mode: MatchMode,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum LexiconTransferFormat {
    Csv,
    Json,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportLexiconInput {
    format: LexiconTransferFormat,
    content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LexiconExport {
    format: LexiconTransferFormat,
    content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalProviderStatus {
    provider_id: &'static str,
    worker_available: bool,
    whisper_server_available: bool,
    active_model_id: Option<String>,
    active_model_validated: bool,
    backend: &'static str,
    detail: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteProviderWorkspace {
    active_profile_id: Option<String>,
    profiles: Vec<RemoteProviderProfileView>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteProviderProfileView {
    id: String,
    display_name: String,
    endpoint: String,
    enabled: bool,
    credential_configured: bool,
    developer_allow_http_loopback: bool,
    connect_timeout_ms: u32,
    request_timeout_ms: u32,
    selected_model_id: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteConnectionInput {
    endpoint: String,
    bearer_token: String,
    developer_allow_http_loopback: bool,
    connect_timeout_ms: u32,
    request_timeout_ms: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveRemoteProviderInput {
    id: Option<String>,
    display_name: String,
    endpoint: String,
    bearer_token: String,
    selected_model_id: String,
    developer_allow_http_loopback: bool,
    connect_timeout_ms: u32,
    request_timeout_ms: u32,
    enabled: bool,
    activate_after_save: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteConnectionTestView {
    healthy: bool,
    detail: String,
    remote_execution: bool,
    max_request_bytes: u64,
    max_audio_duration_ms: u64,
    language_detection: bool,
    word_timestamps: bool,
    models: Vec<RemoteModelView>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteModelView {
    id: String,
    display_name: String,
    installed: bool,
    ready: bool,
}

enum ClipboardPasteAttempt {
    Inserted {
        ticket: free_whisper_platform_windows::ClipboardRestoreTicket,
    },
    CopiedFallback {
        warning: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct DesktopCommandError {
    code: &'static str,
    message: String,
}

impl DesktopCommandError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
fn runtime_snapshot(
    state: State<'_, DesktopState>,
) -> Result<RuntimeSnapshot, DesktopCommandError> {
    let manifest = verified_embedded_manifest();
    let active = active_model_view(&state, manifest.as_ref().ok())?;
    let active_provider = match active_remote_provider(&state)? {
        Some(profile) => Some(ActiveProviderView {
            kind: "remote",
            display_name: profile.display_name,
            model_id: profile.settings.remote_model_id.unwrap_or_default(),
            backend: "remote".to_owned(),
        }),
        None => active.as_ref().map(|model| ActiveProviderView {
            kind: "local",
            display_name: model.display_name.clone(),
            model_id: model.id.clone(),
            backend: model.backend.clone(),
        }),
    };
    let onboarding = onboarding_state(active_provider.is_some());
    let last_transcript = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "transcript database is unavailable"))?
        .transcripts()
        .list_recent(1)
        .map_err(storage_error)?
        .pop()
        .map(transcript_view);
    let (input_devices, microphone_error) = match state.recorder.list_input_devices() {
        Ok(devices) => (devices, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let (recording_settings, recording_settings_error) = load_recording_settings(&state)?;
    let (windows_integration, windows_integration_error) =
        load_windows_integration_settings(&state)?;
    let active_lexicon_profile_id = load_active_lexicon_profile(&state)?.map(|id| id.to_string());
    let hotkey_error = state
        .hotkey_error
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "hotkey state is unavailable"))?
        .clone();
    let capture_phase = state
        .coordinator
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "capture state is unavailable"))?
        .capture_phase();
    let recording = state
        .recording
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "recording state is unavailable"))?
        .as_ref()
        .map(|recording| recording.capture.status());
    let processing = state
        .processing
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "processing state is unavailable"))?
        .is_some();
    let queue_depth = state
        .queued
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "queue state is unavailable"))?
        .as_ref()
        .map_or(0, |_| 1);
    Ok(RuntimeSnapshot {
        app_version: app_version().to_owned(),
        onboarding,
        manifest_available: manifest.is_ok(),
        manifest_error: manifest.err().map(|error| error.message),
        active_model: active,
        active_provider,
        input_devices,
        microphone_error,
        recording_settings,
        recording_settings_error,
        windows_integration,
        windows_integration_error,
        hotkey_error,
        capture_phase: capture_phase.into(),
        active_job_id: capture_phase.job_id().map(|job_id| job_id.to_string()),
        recording,
        processing,
        queue_depth,
        active_lexicon_profile_id,
        last_transcript,
    })
}

#[tauri::command]
fn model_catalog(
    state: State<'_, DesktopState>,
) -> Result<Vec<ModelCatalogItem>, DesktopCommandError> {
    let manifest = verified_embedded_manifest()?;
    let installed = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model database is unavailable"))?
        .models()
        .list()
        .map_err(storage_error)?;
    Ok(manifest
        .manifest()
        .models
        .iter()
        .map(|entry| {
            let record = installed
                .iter()
                .find(|installed| installed.model_id.as_str() == entry.id);
            ModelCatalogItem {
                id: entry.id.clone(),
                display_name: entry.display_name.clone(),
                preset: entry.preset.as_str().to_owned(),
                size_bytes: entry.size_bytes,
                backend: entry.backend.as_str().to_owned(),
                quantization: entry.quantization.clone(),
                estimated_ram_bytes: entry.estimated_ram_bytes,
                license: entry.license.clone(),
                license_url: entry.license_url.clone(),
                source_url: entry.source_url.clone(),
                installed: record.is_some(),
                active: record.is_some_and(|value| value.active),
                validated: record.is_some_and(|value| value.validated_at.is_some()),
            }
        })
        .collect())
}

#[tauri::command]
async fn install_model(
    app: AppHandle,
    state: State<'_, DesktopState>,
    model_id: String,
) -> Result<(), DesktopCommandError> {
    let manifest = verified_embedded_manifest()?;
    let model_id = parse_model_id(model_id)?;
    let cancellation = {
        let mut active = state.downloads.lock().map_err(|_| {
            DesktopCommandError::new("internal", "model download state is unavailable")
        })?;
        if active.contains_key(model_id.as_str()) {
            return Err(DesktopCommandError::new(
                "model_download_active",
                "this model is already downloading",
            ));
        }
        let cancellation = CancellationToken::new();
        active.insert(model_id.as_str().to_owned(), cancellation.clone());
        cancellation
    };
    let manager = ModelManager::new(state.model_root.clone(), embedded_verifier()?)
        .map_err(model_manager_error)?;
    let result = manager
        .install(&manifest, &model_id, &cancellation, |progress| {
            emit_download_progress(&app, progress);
        })
        .await;
    state
        .downloads
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model download state is unavailable"))?
        .remove(model_id.as_str());
    let installed = result.map_err(model_manager_error)?;
    let now = OffsetDateTime::now_utc();
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model database is unavailable"))?
        .models()
        .save(&InstalledModelRecord {
            model_id: installed.model_id,
            provider_id: LOCAL_PROVIDER_ID.to_owned(),
            installation_path: installed.path.to_string_lossy().into_owned(),
            sha256: installed.sha256,
            size_bytes: installed.size_bytes,
            backend: installed.backend,
            installed_at: now,
            validated_at: Some(now),
            active: false,
        })
        .map_err(|error| {
            DesktopCommandError::new(
                "installation_metadata_not_saved",
                format!(
                    "verified model file was installed, but SQLite metadata was not saved: {error}"
                ),
            )
        })
}

#[tauri::command]
fn cancel_model_install(
    state: State<'_, DesktopState>,
    model_id: String,
) -> Result<(), DesktopCommandError> {
    let cancellation = state
        .downloads
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model download state is unavailable"))?
        .get(&model_id)
        .cloned()
        .ok_or_else(|| {
            DesktopCommandError::new("model_download_not_active", "this model is not downloading")
        })?;
    cancellation.cancel();
    Ok(())
}

#[tauri::command]
async fn activate_model(
    state: State<'_, DesktopState>,
    model_id: String,
) -> Result<(), DesktopCommandError> {
    let manifest = verified_embedded_manifest()?;
    let model_id = parse_model_id(model_id)?;
    let manager = ModelManager::new(state.model_root.clone(), embedded_verifier()?)
        .map_err(model_manager_error)?;
    manager
        .validate_installed_model(&manifest, &model_id)
        .await
        .map_err(model_manager_error)?;
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model database is unavailable"))?
        .models()
        .activate_local_cpu(&model_id)
        .map_err(|_| {
            DesktopCommandError::new(
                "model_not_ready",
                "the validated local CPU model is not available for activation",
            )
        })
}

#[tauri::command]
async fn delete_model(
    state: State<'_, DesktopState>,
    model_id: String,
    confirmed_by_user: bool,
) -> Result<(), DesktopCommandError> {
    if !confirmed_by_user {
        return Err(DesktopCommandError::new(
            "delete_confirmation_required",
            "model deletion requires explicit user confirmation",
        ));
    }
    let model_id = parse_model_id(model_id)?;
    let manager = ModelManager::new(state.model_root.clone(), embedded_verifier()?)
        .map_err(model_manager_error)?;
    manager
        .delete(&model_id, DeleteConfirmation::ConfirmedByUser)
        .await
        .map_err(model_manager_error)?;
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model database is unavailable"))?
        .models()
        .delete(&model_id)
        .map_err(storage_error)
}

#[tauri::command]
async fn start_recording(
    app: AppHandle,
    state: State<'_, DesktopState>,
    input: StartRecordingInput,
) -> Result<RecordingStatus, DesktopCommandError> {
    start_recording_inner(&app, &state, input).await
}

fn coordinator_error(error: RecordingCoordinatorError) -> DesktopCommandError {
    match error {
        RecordingCoordinatorError::CaptureBusy { .. } => DesktopCommandError::new(
            "recording_in_progress",
            "Eine Aufnahme wird bereits vorbereitet oder läuft. Bitte diese Aufnahme zuerst beenden.",
        ),
        RecordingCoordinatorError::AlreadyFinalizing { .. } => DesktopCommandError::new(
            "recording_finalizing",
            "Die Aufnahme wird bereits beendet und vorbereitet. Bitte kurz warten.",
        ),
        RecordingCoordinatorError::NoActiveRecording { .. } => DesktopCommandError::new(
            "no_active_recording",
            "Es läuft keine Aufnahme, die beendet werden kann.",
        ),
        RecordingCoordinatorError::JobOwnership { .. } => DesktopCommandError::new(
            "internal",
            "Der Aufnahmezustand konnte nicht sicher zugeordnet werden. Bitte erneut versuchen.",
        ),
    }
}

fn abandon_capture_preparation(state: &DesktopState, job_id: JobId) {
    if let Ok(mut coordinator) = state.coordinator.lock() {
        let _ = coordinator.abandon_preparation(job_id);
    }
}

fn commit_active_recording(
    state: &DesktopState,
    active: ActiveRecording,
) -> Result<Option<ActiveRecording>, DesktopCommandError> {
    let mut coordinator = state
        .coordinator
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "capture state is unavailable"))?;
    let mut recording = state
        .recording
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "recording state is unavailable"))?;
    if recording.is_some() {
        return Err(DesktopCommandError::new(
            "internal",
            "capture slot preparation found an occupied audio handle slot",
        ));
    }
    let commit = coordinator
        .commit_preparation(active.job_id)
        .map_err(coordinator_error)?;
    match commit {
        PreparationCommit::Recording => {
            *recording = Some(active);
            Ok(None)
        }
        PreparationCommit::Cancelled => Ok(Some(active)),
    }
}

fn take_active_for_finalization(
    state: &DesktopState,
) -> Result<ActiveRecording, DesktopCommandError> {
    let mut coordinator = state
        .coordinator
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "capture state is unavailable"))?;
    let job_id = coordinator
        .begin_finalization()
        .map_err(coordinator_error)?;
    let active = state
        .recording
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "recording state is unavailable"))?
        .take();
    let Some(active) = active else {
        let _ = coordinator.release_finalization(job_id);
        return Err(DesktopCommandError::new(
            "internal",
            "capture state was recording without its audio handle",
        ));
    };
    if active.job_id != job_id {
        let _ = coordinator.release_finalization(job_id);
        return Err(DesktopCommandError::new(
            "internal",
            "capture state and audio handle belong to different jobs",
        ));
    }
    Ok(active)
}

fn release_capture_finalization(
    state: &DesktopState,
    job_id: JobId,
) -> Result<(), DesktopCommandError> {
    state
        .coordinator
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "capture state is unavailable"))?
        .release_finalization(job_id)
        .map_err(coordinator_error)
}

#[allow(clippy::needless_borrow)] // Command and tray/hotkey callers share borrowed app state.
async fn start_recording_inner(
    app: &AppHandle,
    state: &DesktopState,
    input: StartRecordingInput,
) -> Result<RecordingStatus, DesktopCommandError> {
    let processing_busy = state
        .processing
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "processing state is unavailable"))?
        .is_some();
    let finalized_job_waiting = state
        .queued
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "queue state is unavailable"))?
        .is_some();
    if processing_busy && finalized_job_waiting {
        return Err(DesktopCommandError::new(
            "queue_full",
            "a transcription and one finalized recording are already pending; wait for a result before recording again",
        ));
    }
    let job_id = JobId::new();
    state
        .coordinator
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "capture state is unavailable"))?
        .reserve_start(job_id)
        .map_err(coordinator_error)?;
    if let Err(error) = persist_recording_settings(&state, &input.settings) {
        abandon_capture_preparation(state, job_id);
        return Err(error);
    }
    let (provider, model_id) = match recording_provider(&app, &state).await {
        Ok(value) => value,
        Err(error) => {
            abandon_capture_preparation(state, job_id);
            return Err(error);
        }
    };
    let mut machine = JobStateMachine::new(job_id);
    if let Err(error) = machine.transition(AppState::PreparingRecording) {
        abandon_capture_preparation(state, job_id);
        return Err(state_error(error));
    }
    emit_state(&app, job_id, AppState::PreparingRecording, None);
    let load_started = Instant::now();
    if let Err(error) = provider.ensure_model_ready(&model_id).await {
        abandon_capture_preparation(state, job_id);
        let failure = shutdown_after_failure(&provider, provider_error(error)).await;
        emit_state(
            &app,
            job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    let model_load_ms = elapsed_ms(load_started);
    let capture = match state.recorder.start(input.settings) {
        Ok(capture) => capture,
        Err(error) => {
            abandon_capture_preparation(state, job_id);
            let failure = shutdown_after_failure(&provider, audio_error(error)).await;
            emit_state(
                &app,
                job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            return Err(failure);
        }
    };
    if let Err(error) = machine.transition(AppState::Recording) {
        abandon_capture_preparation(state, job_id);
        let primary = match capture.discard().map_err(audio_error) {
            Ok(()) => state_error(error),
            Err(discard) => combine_cleanup_failure(state_error(error), discard),
        };
        let failure = shutdown_after_failure(&provider, primary).await;
        emit_state(
            &app,
            job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    let status = capture.status();
    let language = input
        .language
        .filter(|language| !language.trim().is_empty() && language != "auto")
        .map(LanguageSelection::Explicit)
        .unwrap_or(LanguageSelection::Auto);
    let target_window = match state.windows.capture_target_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            emit_platform_error(
                &app,
                "target_window",
                format!(
                    "The original target window could not be captured; paste will require a safe fallback: {error}"
                ),
            );
            None
        }
    };
    let active = ActiveRecording {
        job_id,
        machine,
        language,
        capture,
        provider: Arc::clone(&provider),
        model_load_ms,
        target_window,
    };
    let cancelled_capture = match commit_active_recording(state, active) {
        Ok(cancelled_capture) => cancelled_capture,
        Err(error) => {
            abandon_capture_preparation(state, job_id);
            let failure = shutdown_after_failure(&provider, error).await;
            emit_state(
                &app,
                job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            return Err(failure);
        }
    };
    if let Some(cancelled_capture) = cancelled_capture {
        let primary = match cancelled_capture.capture.discard().map_err(audio_error) {
            Ok(()) => DesktopCommandError::new(
                "transcription_cancelled",
                "Die vorbereitete Aufnahme wurde abgebrochen.",
            ),
            Err(discard) => discard,
        };
        let failure = shutdown_after_failure(&provider, primary).await;
        emit_state(&app, job_id, AppState::Cancelling, None);
        emit_state(&app, job_id, AppState::Idle, None);
        return Err(failure);
    }
    emit_state(&app, job_id, AppState::Recording, None);
    Ok(status)
}

#[tauri::command]
fn recording_status(
    state: State<'_, DesktopState>,
) -> Result<Option<RecordingStatus>, DesktopCommandError> {
    Ok(state
        .recording
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "recording state is unavailable"))?
        .as_ref()
        .map(|recording| recording.capture.status()))
}

#[tauri::command]
async fn stop_recording(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<TranscriptView, DesktopCommandError> {
    stop_recording_inner(&app, &state).await
}

#[allow(clippy::needless_borrow)] // Keeps the shared controller body auditable against its command predecessor.
async fn stop_recording_inner(
    app: &AppHandle,
    state: &DesktopState,
) -> Result<TranscriptView, DesktopCommandError> {
    let mut active = take_active_for_finalization(state)?;
    if let Err(error) = active.machine.transition(AppState::FinalizingAudio) {
        let _ = release_capture_finalization(state, active.job_id);
        return Err(state_error(error));
    }
    emit_state(&app, active.job_id, AppState::FinalizingAudio, None);
    let finalized = match active.capture.finish() {
        Ok(value) => value,
        Err(error) => {
            let release = release_capture_finalization(state, active.job_id);
            let primary = audio_error(error);
            let primary = match release {
                Ok(()) => primary,
                Err(release) => combine_cleanup_failure(primary, release),
            };
            let failure = shutdown_after_failure(&active.provider, primary).await;
            emit_state(
                &app,
                active.job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            return Err(failure);
        }
    };
    if let Err(error) = release_capture_finalization(state, active.job_id) {
        let failure = shutdown_after_failure(&active.provider, error).await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    if !finalized.speech.has_minimum_speech {
        let failure = shutdown_after_failure(
            &active.provider,
            DesktopCommandError::new(
                "empty_recording",
                "No speech was detected; nothing was sent to the transcription model.",
            ),
        )
        .await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    if let Err(error) = active.machine.transition(AppState::Queued) {
        let failure = shutdown_after_failure(&active.provider, state_error(error)).await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    emit_state(&app, active.job_id, AppState::Queued, None);
    let (prompt_terms, correction_rules) = match lexicon_snapshot(&state) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let failure = shutdown_after_failure(&active.provider, error).await;
            emit_state(
                &app,
                active.job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            return Err(failure);
        }
    };
    let request_id = RequestId::new();
    if let Err(error) = reserve_processing_slot(
        &state,
        active.job_id,
        request_id,
        Arc::clone(&active.provider),
    )
    .await
    {
        let failure = shutdown_after_failure(&active.provider, error).await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    if let Err(error) = active.machine.transition(AppState::LoadingModel) {
        let failure = release_processing_after_failure(
            state,
            active.job_id,
            &active.provider,
            state_error(error),
        )
        .await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    emit_state(&app, active.job_id, AppState::LoadingModel, None);
    if let Err(error) = active.machine.transition(AppState::Transcribing) {
        let failure = release_processing_after_failure(
            state,
            active.job_id,
            &active.provider,
            state_error(error),
        )
        .await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    emit_state(&app, active.job_id, AppState::Transcribing, None);

    let audio_duration_ms = finalized.audio.duration_ms();
    let started = Instant::now();
    let raw_result = active
        .provider
        .transcribe(TranscriptionRequest {
            id: request_id,
            audio: finalized.audio,
            language: active.language.clone(),
            decoding: DecodingOptions::default(),
            prompt_terms,
        })
        .await;
    let inference_ms = elapsed_ms(started);
    let mut result = match raw_result {
        Ok(result) => result,
        Err(error) => {
            let failure = release_processing_after_failure(
                &state,
                active.job_id,
                &active.provider,
                provider_error(error),
            )
            .await;
            emit_state(
                &app,
                active.job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            return Err(failure);
        }
    };
    if let Err(error) = active.machine.transition(AppState::ApplyingCorrections) {
        let failure = release_processing_after_failure(
            &state,
            active.job_id,
            &active.provider,
            state_error(error),
        )
        .await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    emit_state(&app, active.job_id, AppState::ApplyingCorrections, None);
    let correction_started = Instant::now();
    let corrections = apply_corrections(
        &result.raw_text,
        &correction_rules,
        OffsetDateTime::now_utc(),
    )
    .map_err(|error| DesktopCommandError::new("correction_failed", error.to_string()));
    let corrections = match corrections {
        Ok(corrections) => corrections,
        Err(error) => {
            let failure =
                release_processing_after_failure(&state, active.job_id, &active.provider, error)
                    .await;
            emit_state(
                &app,
                active.job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            return Err(failure);
        }
    };
    let correction_ms = elapsed_ms(correction_started);
    result.final_text = corrections.final_text;
    result.corrections = corrections.corrections;
    result.durations = ProcessingDurations {
        queue_ms: 0,
        model_load_ms: active.model_load_ms,
        inference_ms,
        correction_ms,
    };
    let processing_ms = result
        .durations
        .queue_ms
        .saturating_add(result.durations.model_load_ms)
        .saturating_add(result.durations.inference_ms)
        .saturating_add(result.durations.correction_ms);
    let (windows_integration, _) = load_windows_integration_settings(&state)?;
    let auto_paste_requested = windows_integration.auto_paste && active.target_window.is_some();
    let auto_attempt = if auto_paste_requested {
        let text = result.final_text.clone();
        let target_window = active.target_window.clone().expect("checked above");
        Some(
            tokio::task::spawn_blocking(move || copy_and_paste(text, target_window, false))
                .await
                .map_err(|error| DesktopCommandError::new("paste_failed", error.to_string()))?,
        )
    } else {
        None
    };
    let mut auto_restore_ticket = None;
    let mut auto_paste_inserted = false;
    let mut injection_outcome = InjectionOutcome::NeedsUserConfirmation {
        reason: "Text is ready; copy it explicitly to the clipboard.".to_owned(),
    };
    if let Some(attempt) = auto_attempt {
        match attempt {
            ClipboardPasteAttempt::Inserted { ticket } => {
                auto_restore_ticket = Some(ticket);
                auto_paste_inserted = true;
                injection_outcome = InjectionOutcome::Inserted;
            }
            ClipboardPasteAttempt::CopiedFallback { warning } => {
                result.warnings.push(warning);
                injection_outcome = InjectionOutcome::CopiedToClipboard;
            }
            ClipboardPasteAttempt::Failed { reason } => {
                result
                    .warnings
                    .push(format!("Auto-paste could not start: {reason}"));
                injection_outcome = InjectionOutcome::FailedWithReason { reason };
            }
        }
    }
    let stored = StoredTranscript {
        id: TranscriptId::new(),
        created_at: OffsetDateTime::now_utc(),
        raw_text: result.raw_text,
        final_text: result.final_text,
        language_requested: language_requested(&active.language),
        language_detected: result.language_detected,
        language_confidence_milli: result.language_confidence_milli,
        provider_id: result.provider.provider_id.clone(),
        model_id: result.provider.model_id.clone(),
        audio_duration_ms,
        processing_ms,
        injection_outcome,
        target_window: windows_integration
            .auto_paste
            .then(|| active.target_window.clone())
            .flatten(),
        provider_metadata: Some(result.provider),
        segments: result.segments,
        warnings: result.warnings,
        durations: result.durations,
        corrections: result
            .corrections
            .into_iter()
            .map(|correction| StoredCorrection {
                correction,
                reverted_at: None,
            })
            .collect(),
    };
    let saved = match state.database.lock() {
        Ok(mut database) => database.transcripts().save(&stored).map_err(storage_error),
        Err(_) => Err(DesktopCommandError::new(
            "internal",
            "transcript database is unavailable",
        )),
    };
    if let Err(error) = saved {
        let failure =
            release_processing_after_failure(&state, active.job_id, &active.provider, error).await;
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(failure.message.clone()),
        );
        return Err(failure);
    }
    if let Err(error) = release_processing(&state, active.job_id, &active.provider).await {
        emit_state(
            &app,
            active.job_id,
            AppState::Failed,
            Some(error.message.clone()),
        );
        return Err(error);
    }
    if auto_paste_inserted {
        active
            .machine
            .transition(AppState::Injecting)
            .map_err(state_error)?;
        emit_state(&app, active.job_id, AppState::Injecting, None);
        active
            .machine
            .transition(AppState::Completed)
            .map_err(state_error)?;
        emit_state(&app, active.job_id, AppState::Completed, None);
        if windows_integration.restore_clipboard_after_paste
            && let Some(ticket) = auto_restore_ticket
        {
            schedule_clipboard_restore(app.clone(), stored.id, ticket);
        }
    } else {
        active
            .machine
            .transition(AppState::AwaitingInjectionConfirmation)
            .map_err(state_error)?;
        emit_state(
            &app,
            active.job_id,
            AppState::AwaitingInjectionConfirmation,
            None,
        );
    }
    Ok(transcript_view(stored))
}

#[tauri::command]
async fn cancel_current_job(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), DesktopCommandError> {
    cancel_current_job_inner(&app, &state).await
}

#[allow(clippy::needless_borrow)] // Same shared controller is called by command, tray and hotkey routes.
async fn cancel_current_job_inner(
    app: &AppHandle,
    state: &DesktopState,
) -> Result<(), DesktopCommandError> {
    let cancelled_capture = state
        .coordinator
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "capture state is unavailable"))?
        .cancel_capture();
    if let Some(cancelled_capture) = cancelled_capture {
        match cancelled_capture {
            CancelledCapture::Preparing { job_id } => {
                emit_state(&app, job_id, AppState::Cancelling, None);
                emit_state(&app, job_id, AppState::Idle, None);
                return Ok(());
            }
            CancelledCapture::Recording { job_id } => {
                let active = state
                    .recording
                    .lock()
                    .map_err(|_| {
                        DesktopCommandError::new("internal", "recording state is unavailable")
                    })?
                    .take()
                    .ok_or_else(|| {
                        DesktopCommandError::new(
                            "internal",
                            "capture state was recording without its audio handle",
                        )
                    })?;
                if active.job_id != job_id {
                    return Err(DesktopCommandError::new(
                        "internal",
                        "capture cancellation belongs to a different audio handle",
                    ));
                }
                emit_state(&app, job_id, AppState::Cancelling, None);
                let discard = active.capture.discard().map_err(audio_error);
                let shutdown = active.provider.shutdown().await.map_err(provider_error);
                match (discard, shutdown) {
                    (Ok(()), Ok(())) => {
                        emit_state(&app, job_id, AppState::Idle, None);
                        return Ok(());
                    }
                    (Err(discard), Ok(())) => {
                        emit_state(
                            &app,
                            job_id,
                            AppState::Failed,
                            Some(discard.message.clone()),
                        );
                        return Err(discard);
                    }
                    (Ok(()), Err(shutdown)) => {
                        emit_state(
                            &app,
                            job_id,
                            AppState::Failed,
                            Some(shutdown.message.clone()),
                        );
                        return Err(shutdown);
                    }
                    (Err(discard), Err(shutdown)) => {
                        let failure = combine_cleanup_failure(discard, shutdown);
                        emit_state(
                            &app,
                            job_id,
                            AppState::Failed,
                            Some(failure.message.clone()),
                        );
                        return Err(failure);
                    }
                }
            }
            CancelledCapture::Finalizing { .. } => {
                return Err(DesktopCommandError::new(
                    "recording_finalizing",
                    "Die Aufnahme wird bereits beendet. Bitte kurz warten.",
                ));
            }
        }
    }
    let processing = state
        .processing
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "processing state is unavailable"))?
        .clone()
        .ok_or_else(|| {
            DesktopCommandError::new(
                "no_active_job",
                "there is no active recording or transcription",
            )
        })?;
    if let Some(queued_job) = cancel_queued_job(state)? {
        emit_state(&app, queued_job, AppState::Cancelling, None);
    }
    emit_state(&app, processing.job_id, AppState::Cancelling, None);
    let cancellation = processing.provider.cancel(processing.request_id).await;
    let release = release_processing(&state, processing.job_id, &processing.provider).await;
    match (cancellation, release) {
        (Ok(()), Ok(())) => {
            emit_state(&app, processing.job_id, AppState::Idle, None);
            Ok(())
        }
        (Err(error), Ok(())) => {
            let failure = provider_error(error);
            emit_state(
                &app,
                processing.job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            Err(failure)
        }
        (Ok(()), Err(error)) => {
            emit_state(
                &app,
                processing.job_id,
                AppState::Failed,
                Some(error.message.clone()),
            );
            Err(error)
        }
        (Err(cancellation), Err(release)) => {
            let failure = combine_cleanup_failure(provider_error(cancellation), release);
            emit_state(
                &app,
                processing.job_id,
                AppState::Failed,
                Some(failure.message.clone()),
            );
            Err(failure)
        }
    }
}

#[tauri::command]
async fn copy_transcript(
    state: State<'_, DesktopState>,
    transcript_id: String,
) -> Result<(), DesktopCommandError> {
    let transcript_id = parse_transcript_id(&transcript_id)?;
    let text = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "transcript database is unavailable"))?
        .transcripts()
        .get(transcript_id)
        .map_err(storage_error)?
        .ok_or_else(|| {
            DesktopCommandError::new("transcript_not_found", "transcript was not found")
        })?
        .final_text;
    let outcome = tokio::task::spawn_blocking(move || {
        WindowsClipboard
            .copy_text(&text)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| DesktopCommandError::new("clipboard_failed", error.to_string()))?;
    match outcome {
        Ok(()) => state
            .database
            .lock()
            .map_err(|_| {
                DesktopCommandError::new("internal", "transcript database is unavailable")
            })?
            .transcripts()
            .update_injection_outcome(transcript_id, &InjectionOutcome::CopiedToClipboard)
            .map_err(storage_error),
        Err(message) => {
            let outcome = InjectionOutcome::FailedWithReason {
                reason: message.clone(),
            };
            let persisted = state
                .database
                .lock()
                .map_err(|_| {
                    DesktopCommandError::new("internal", "transcript database is unavailable")
                })?
                .transcripts()
                .update_injection_outcome(transcript_id, &outcome);
            match persisted {
                Ok(()) => Err(DesktopCommandError::new("clipboard_failed", message)),
                Err(error) => Err(DesktopCommandError::new(
                    "clipboard_failure_not_persisted",
                    format!(
                        "clipboard failed: {message}; the failure outcome could not be stored: {error}"
                    ),
                )),
            }
        }
    }
}

fn copy_and_paste(
    text: String,
    target_window: TargetWindowSnapshot,
    confirmed_by_user: bool,
) -> ClipboardPasteAttempt {
    let clipboard = WindowsClipboard;
    let ticket = match clipboard.copy_with_restore_ticket(&text) {
        Ok(ticket) => ticket,
        Err(error) => {
            return ClipboardPasteAttempt::Failed {
                reason: format!("The transcript could not be copied before paste: {error}"),
            };
        }
    };
    match WindowsDesktop.paste_into_original(&target_window, confirmed_by_user) {
        Ok(()) => ClipboardPasteAttempt::Inserted { ticket },
        Err(PasteRefusalOrError::Refused(reason)) => ClipboardPasteAttempt::CopiedFallback {
            warning: format!(
                "Paste was blocked by the Windows safety policy ({reason:?}); the transcript remains in the clipboard."
            ),
        },
        Err(PasteRefusalOrError::Error(error)) => ClipboardPasteAttempt::CopiedFallback {
            warning: format!(
                "Paste could not be completed ({error}); the transcript remains in the clipboard."
            ),
        },
    }
}

fn schedule_clipboard_restore(
    app: AppHandle,
    transcript_id: TranscriptId,
    ticket: free_whisper_platform_windows::ClipboardRestoreTicket,
) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let restore =
            tokio::task::spawn_blocking(move || WindowsClipboard.restore_if_unchanged(ticket))
                .await;
        let warning = match restore {
            Ok(ClipboardRestoreOutcome::Restored) => None,
            Ok(ClipboardRestoreOutcome::NotRestorable) => Some(
                "The previous clipboard content was not plain Unicode text and was not restored."
                    .to_owned(),
            ),
            Ok(ClipboardRestoreOutcome::SkippedSequenceChanged) => Some(
                "Clipboard changed after paste, so the app did not restore its previous content."
                    .to_owned(),
            ),
            Ok(ClipboardRestoreOutcome::Failed(message)) => Some(format!(
                "The previous clipboard text could not be restored: {message}"
            )),
            Err(error) => Some(format!("The clipboard restore task could not run: {error}")),
        };
        if let Some(warning) = warning {
            let state = app.state::<DesktopState>();
            let persisted = state
                .database
                .lock()
                .map_err(|_| {
                    DesktopCommandError::new("internal", "transcript database is unavailable")
                })
                .and_then(|mut database| {
                    database
                        .transcripts()
                        .append_warning(transcript_id, &warning)
                        .map_err(storage_error)
                });
            if let Err(error) = persisted {
                emit_platform_error(
                    &app,
                    "clipboard_restore",
                    format!("{warning} It could not be persisted: {}", error.message),
                );
            } else {
                emit_platform_error(&app, "clipboard_restore", warning);
            }
        }
    });
}

#[tauri::command]
async fn paste_transcript_to_original(
    app: AppHandle,
    state: State<'_, DesktopState>,
    transcript_id: String,
) -> Result<TranscriptView, DesktopCommandError> {
    let transcript_id = parse_transcript_id(&transcript_id)?;
    let transcript = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "transcript database is unavailable"))?
        .transcripts()
        .get(transcript_id)
        .map_err(storage_error)?
        .ok_or_else(|| {
            DesktopCommandError::new("transcript_not_found", "transcript was not found")
        })?;
    let target_window = transcript.target_window.clone().ok_or_else(|| {
        DesktopCommandError::new(
            "original_window_not_available",
            "this result has no opt-in original-window snapshot; copy it explicitly instead",
        )
    })?;
    let attempt = tokio::task::spawn_blocking(move || {
        copy_and_paste(transcript.final_text, target_window, true)
    })
    .await
    .map_err(|error| DesktopCommandError::new("paste_failed", error.to_string()))?;
    match attempt {
        ClipboardPasteAttempt::Inserted { ticket } => {
            state
                .database
                .lock()
                .map_err(|_| {
                    DesktopCommandError::new("internal", "transcript database is unavailable")
                })?
                .transcripts()
                .update_injection_outcome(transcript_id, &InjectionOutcome::Inserted)
                .map_err(storage_error)?;
            let (settings, _) = load_windows_integration_settings(&state)?;
            if settings.restore_clipboard_after_paste {
                schedule_clipboard_restore(app, transcript_id, ticket);
            }
        }
        ClipboardPasteAttempt::CopiedFallback { warning } => {
            let mut database = state.database.lock().map_err(|_| {
                DesktopCommandError::new("internal", "transcript database is unavailable")
            })?;
            database
                .transcripts()
                .update_injection_outcome(transcript_id, &InjectionOutcome::CopiedToClipboard)
                .map_err(storage_error)?;
            database
                .transcripts()
                .append_warning(transcript_id, &warning)
                .map_err(storage_error)?;
        }
        ClipboardPasteAttempt::Failed { reason } => {
            let outcome = InjectionOutcome::FailedWithReason {
                reason: reason.clone(),
            };
            state
                .database
                .lock()
                .map_err(|_| {
                    DesktopCommandError::new("internal", "transcript database is unavailable")
                })?
                .transcripts()
                .update_injection_outcome(transcript_id, &outcome)
                .map_err(storage_error)?;
            return Err(DesktopCommandError::new("paste_failed", reason));
        }
    }
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "transcript database is unavailable"))?
        .transcripts()
        .get(transcript_id)
        .map_err(storage_error)?
        .map(transcript_view)
        .ok_or_else(|| DesktopCommandError::new("transcript_not_found", "transcript was not found"))
}

#[tauri::command]
fn recent_transcripts(
    state: State<'_, DesktopState>,
) -> Result<Vec<TranscriptView>, DesktopCommandError> {
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "transcript database is unavailable"))?
        .transcripts()
        .list_recent(25)
        .map(|items| items.into_iter().map(transcript_view).collect())
        .map_err(storage_error)
}

#[tauri::command]
fn search_transcripts(
    state: State<'_, DesktopState>,
    query: String,
) -> Result<Vec<TranscriptView>, DesktopCommandError> {
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "transcript database is unavailable"))?;
    let transcripts = if query.trim().is_empty() {
        database.transcripts().list_recent(100)
    } else {
        database.transcripts().search(&query, 100)
    }
    .map_err(storage_error)?;
    Ok(transcripts.into_iter().map(transcript_view).collect())
}

#[tauri::command]
fn revert_transcript_correction(
    state: State<'_, DesktopState>,
    transcript_id: String,
    correction_id: String,
) -> Result<TranscriptView, DesktopCommandError> {
    let transcript_id = parse_transcript_id(&transcript_id)?;
    let correction_id = parse_transcript_correction_id(&correction_id)?;
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "transcript database is unavailable"))?;
    database
        .transcripts()
        .revert_correction(transcript_id, correction_id, OffsetDateTime::now_utc())
        .map_err(storage_error)?;
    database
        .transcripts()
        .get(transcript_id)
        .map_err(storage_error)?
        .map(transcript_view)
        .ok_or_else(|| DesktopCommandError::new("transcript_not_found", "transcript was not found"))
}

#[tauri::command]
fn lexicon_workspace(
    state: State<'_, DesktopState>,
) -> Result<LexiconWorkspaceView, DesktopCommandError> {
    let active_profile_id = load_active_lexicon_profile(&state)?;
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?;
    let profiles = database
        .lexicon()
        .profiles()
        .map_err(storage_error)?
        .into_iter()
        .map(|profile| LexiconProfileView {
            id: profile.id.to_string(),
            name: profile.name,
            enabled: profile.enabled,
        })
        .collect();
    let entries = database.lexicon().entries().map_err(storage_error)?;
    let mut entry_views = Vec::with_capacity(entries.len());
    for entry in entries {
        let variants = database
            .lexicon()
            .variants_for(entry.id)
            .map_err(storage_error)?
            .into_iter()
            .map(|variant| LexiconVariantView {
                id: variant.id.to_string(),
                variant_text: variant.variant_text,
                match_mode: variant.match_mode,
            })
            .collect();
        let profile_id = match entry.scope {
            LexiconScope::Global => None,
            LexiconScope::Profile(id) => Some(id.to_string()),
        };
        entry_views.push(LexiconEntryView {
            id: entry.id.to_string(),
            canonical_text: entry.canonical_text,
            language: entry.language,
            category: entry.category,
            priority: entry.priority,
            enabled: entry.enabled,
            profile_id,
            variants,
        });
    }
    Ok(LexiconWorkspaceView {
        active_profile_id: active_profile_id.map(|id| id.to_string()),
        profiles,
        entries: entry_views,
    })
}

#[tauri::command]
fn save_lexicon_profile(
    state: State<'_, DesktopState>,
    input: SaveLexiconProfileInput,
) -> Result<LexiconWorkspaceView, DesktopCommandError> {
    let now = OffsetDateTime::now_utc();
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?;
    let mut profile = LexiconProfile::new(&input.name, now)
        .map_err(|error| DesktopCommandError::new("lexicon_profile_invalid", error.to_string()))?;
    profile.enabled = input.enabled;
    if let Some(id) = input.id {
        let id = parse_lexicon_profile_id(&id)?;
        let existing = database
            .lexicon()
            .profiles()
            .map_err(storage_error)?
            .into_iter()
            .find(|candidate| candidate.id == id)
            .ok_or_else(|| {
                DesktopCommandError::new(
                    "lexicon_profile_not_found",
                    "lexicon profile was not found",
                )
            })?;
        profile.id = id;
        profile.created_at = existing.created_at;
    }
    database
        .lexicon()
        .save_profile(&profile)
        .map_err(storage_error)?;
    drop(database);
    lexicon_workspace(state)
}

#[tauri::command]
fn delete_lexicon_profile(
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<LexiconWorkspaceView, DesktopCommandError> {
    let profile_id = parse_lexicon_profile_id(&profile_id)?;
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?;
    let active = database
        .settings()
        .get(ACTIVE_LEXICON_PROFILE_SETTINGS_KEY)
        .map_err(storage_error)?;
    database
        .lexicon()
        .delete_profile(profile_id)
        .map_err(storage_error)?;
    if active == Some(serde_json::Value::String(profile_id.to_string())) {
        database
            .settings()
            .set(
                ACTIVE_LEXICON_PROFILE_SETTINGS_KEY,
                &serde_json::Value::Null,
                OffsetDateTime::now_utc(),
            )
            .map_err(storage_error)?;
    }
    drop(database);
    lexicon_workspace(state)
}

#[tauri::command]
fn save_lexicon_entry(
    state: State<'_, DesktopState>,
    input: SaveLexiconEntryInput,
) -> Result<LexiconWorkspaceView, DesktopCommandError> {
    let now = OffsetDateTime::now_utc();
    let scope = input
        .profile_id
        .as_deref()
        .map(parse_lexicon_profile_id)
        .transpose()?
        .map_or(LexiconScope::Global, LexiconScope::Profile);
    let mut entry = LexiconEntry::new(
        &input.canonical_text,
        input.language.as_deref(),
        input.category.as_deref(),
        input.priority,
        scope,
        now,
    )
    .map_err(|error| DesktopCommandError::new("lexicon_entry_invalid", error.to_string()))?;
    entry.enabled = input.enabled;
    let variants = input
        .variants
        .iter()
        .map(|variant| {
            LexiconVariant::new(entry.id, &variant.variant_text, variant.match_mode, now)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| DesktopCommandError::new("lexicon_variant_invalid", error.to_string()))?;
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?
        .lexicon()
        .save_entry(&entry, &variants)
        .map_err(storage_error)?;
    lexicon_workspace(state)
}

#[tauri::command]
fn delete_lexicon_entry(
    state: State<'_, DesktopState>,
    entry_id: String,
) -> Result<LexiconWorkspaceView, DesktopCommandError> {
    let entry_id = parse_lexicon_entry_id(&entry_id)?;
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?
        .lexicon()
        .delete_entry(entry_id)
        .map_err(storage_error)?;
    lexicon_workspace(state)
}

#[tauri::command]
fn set_active_lexicon_profile(
    state: State<'_, DesktopState>,
    profile_id: Option<String>,
) -> Result<LexiconWorkspaceView, DesktopCommandError> {
    let profile_id = profile_id
        .as_deref()
        .map(parse_lexicon_profile_id)
        .transpose()?;
    persist_active_lexicon_profile(&state, profile_id)?;
    lexicon_workspace(state)
}

#[tauri::command]
fn import_lexicon(
    state: State<'_, DesktopState>,
    input: ImportLexiconInput,
) -> Result<LexiconImportReport, DesktopCommandError> {
    let drafts = match input.format {
        LexiconTransferFormat::Csv => parse_lexicon_csv(&input.content),
        LexiconTransferFormat::Json => parse_lexicon_json(&input.content),
    }
    .map_err(storage_error)?;
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?
        .lexicon()
        .import(&drafts, OffsetDateTime::now_utc())
        .map_err(storage_error)
}

#[tauri::command]
fn export_lexicon(
    state: State<'_, DesktopState>,
    format: LexiconTransferFormat,
) -> Result<LexiconExport, DesktopCommandError> {
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?;
    let entries = database.lexicon().entries().map_err(storage_error)?;
    let mut drafts = Vec::with_capacity(entries.len());
    for entry in entries {
        let variants = database
            .lexicon()
            .variants_for(entry.id)
            .map_err(storage_error)?
            .into_iter()
            .map(|variant| LexiconVariantDraft {
                variant_text: variant.variant_text,
                match_mode: variant.match_mode,
            })
            .collect();
        drafts.push(LexiconEntryDraft {
            canonical_text: entry.canonical_text,
            language: entry.language,
            category: entry.category,
            priority: entry.priority,
            enabled: entry.enabled,
            scope: entry.scope,
            variants,
        });
    }
    let content = match format {
        LexiconTransferFormat::Json => serde_json::to_string_pretty(&drafts).map_err(|error| {
            DesktopCommandError::new("lexicon_export_failed", error.to_string())
        })?,
        LexiconTransferFormat::Csv => lexicon_csv(&drafts),
    };
    Ok(LexiconExport { format, content })
}

#[tauri::command]
fn local_provider_status(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<LocalProviderStatus, DesktopCommandError> {
    let model = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model database is unavailable"))?
        .models()
        .active_local_cpu()
        .map_err(storage_error)?;
    let sidecars = locate_sidecars(&app);
    let (worker_available, whisper_server_available, detail) = match sidecars {
        Ok(paths) => (
            paths.worker.is_file(),
            paths.server.is_file(),
            "Local worker sidecars are packaged and will start only for a recording.".to_owned(),
        ),
        Err(error) => (false, false, error.message),
    };
    Ok(LocalProviderStatus {
        provider_id: LOCAL_PROVIDER_ID,
        worker_available,
        whisper_server_available,
        active_model_id: model.as_ref().map(|item| item.model_id.as_str().to_owned()),
        active_model_validated: model.is_some_and(|item| item.validated_at.is_some()),
        backend: "cpu",
        detail,
    })
}

#[tauri::command]
fn remote_provider_workspace(
    state: State<'_, DesktopState>,
) -> Result<RemoteProviderWorkspace, DesktopCommandError> {
    let active_profile_id = active_remote_profile_id(&state)?;
    let profiles = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?
        .providers()
        .list()
        .map_err(storage_error)?
        .into_iter()
        .filter(|profile| profile.provider_id == REMOTE_PROVIDER_ID)
        .map(remote_profile_view)
        .collect();
    Ok(RemoteProviderWorkspace {
        active_profile_id,
        profiles,
    })
}

#[tauri::command]
async fn test_remote_provider(
    input: RemoteConnectionInput,
) -> Result<RemoteConnectionTestView, DesktopCommandError> {
    probe_remote_provider(&input).await
}

#[tauri::command]
async fn save_remote_provider(
    state: State<'_, DesktopState>,
    input: SaveRemoteProviderInput,
) -> Result<RemoteProviderWorkspace, DesktopCommandError> {
    let connection = RemoteConnectionInput {
        endpoint: input.endpoint.clone(),
        bearer_token: input.bearer_token.clone(),
        developer_allow_http_loopback: input.developer_allow_http_loopback,
        connect_timeout_ms: input.connect_timeout_ms,
        request_timeout_ms: input.request_timeout_ms,
    };
    let probe = probe_remote_provider(&connection).await?;
    if !probe.healthy || !probe.remote_execution {
        return Err(DesktopCommandError::new(
            "remote_capability_mismatch",
            "the worker is not ready for remote execution",
        ));
    }
    if !probe
        .models
        .iter()
        .any(|model| model.id == input.selected_model_id && model.installed && model.ready)
    {
        return Err(DesktopCommandError::new(
            "remote_model_not_ready",
            "select a ready model reported by this worker",
        ));
    }
    let model_id = parse_model_id(input.selected_model_id.clone())?;
    let profile_id = input
        .id
        .map(parse_provider_profile_id)
        .transpose()?
        .unwrap_or_else(ProviderProfileId::new);
    let existing = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?
        .providers()
        .get(profile_id)
        .map_err(storage_error)?;
    if existing
        .as_ref()
        .is_some_and(|profile| profile.provider_id != REMOTE_PROVIDER_ID)
    {
        return Err(DesktopCommandError::new(
            "provider_profile_invalid",
            "the selected profile is not a remote-worker profile",
        ));
    }
    let credential_reference = existing
        .as_ref()
        .and_then(|profile| profile.credential_reference.clone())
        .unwrap_or_else(|| format!("{CREDENTIAL_REFERENCE_PREFIX}{profile_id}"));
    let created_credential_reference = existing
        .as_ref()
        .and_then(|profile| profile.credential_reference.as_ref())
        .is_none();
    state
        .credential_store
        .write_secret(&credential_reference, &input.bearer_token)
        .map_err(secret_store_error)?;
    let now = OffsetDateTime::now_utc();
    let profile = ProviderProfileRecord {
        id: profile_id,
        provider_id: REMOTE_PROVIDER_ID.to_owned(),
        display_name: input.display_name,
        endpoint: Some(input.endpoint),
        credential_reference: Some(credential_reference),
        settings: ProviderProfileSettings {
            connect_timeout_ms: input.connect_timeout_ms,
            request_timeout_ms: input.request_timeout_ms,
            developer_allow_http_loopback: input.developer_allow_http_loopback,
            remote_model_id: Some(model_id.as_str().to_owned()),
        },
        enabled: input.enabled,
        created_at: existing.as_ref().map_or(now, |value| value.created_at),
        updated_at: now,
    };
    let persist = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?
        .providers()
        .save(&profile)
        .map_err(storage_error);
    if let Err(error) = persist {
        if created_credential_reference {
            let _ = state.credential_store.delete_secret(
                profile
                    .credential_reference
                    .as_deref()
                    .expect("credential reference is always set"),
            );
        }
        return Err(error);
    }
    if input.activate_after_save {
        activate_remote_profile(&state, profile_id)?;
    }
    remote_provider_workspace(state)
}

#[tauri::command]
fn activate_remote_provider(
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<RemoteProviderWorkspace, DesktopCommandError> {
    activate_remote_profile(&state, parse_provider_profile_id(profile_id)?)?;
    remote_provider_workspace(state)
}

#[tauri::command]
fn use_local_provider(
    state: State<'_, DesktopState>,
) -> Result<RemoteProviderWorkspace, DesktopCommandError> {
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?
        .settings()
        .set(
            ACTIVE_REMOTE_PROVIDER_SETTINGS_KEY,
            &serde_json::Value::Null,
            OffsetDateTime::now_utc(),
        )
        .map_err(storage_error)?;
    remote_provider_workspace(state)
}

#[tauri::command]
fn delete_remote_provider(
    state: State<'_, DesktopState>,
    profile_id: String,
    confirmed_by_user: bool,
) -> Result<RemoteProviderWorkspace, DesktopCommandError> {
    if !confirmed_by_user {
        return Err(DesktopCommandError::new(
            "delete_confirmation_required",
            "remote provider deletion requires explicit user confirmation",
        ));
    }
    let profile_id = parse_provider_profile_id(profile_id)?;
    let profile = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?
        .providers()
        .get(profile_id)
        .map_err(storage_error)?
        .ok_or_else(|| {
            DesktopCommandError::new("provider_profile_missing", "provider not found")
        })?;
    if profile.provider_id != REMOTE_PROVIDER_ID {
        return Err(DesktopCommandError::new(
            "provider_profile_invalid",
            "only remote-worker profiles can be removed here",
        ));
    }
    if let Some(reference) = profile.credential_reference.as_deref() {
        state
            .credential_store
            .delete_secret(reference)
            .map_err(secret_store_error)?;
    }
    {
        let mut database = state.database.lock().map_err(|_| {
            DesktopCommandError::new("internal", "provider database is unavailable")
        })?;
        database
            .providers()
            .delete(profile_id)
            .map_err(storage_error)?;
        if active_remote_profile_id_from_database(&mut database)?
            .is_some_and(|active| active == profile_id)
        {
            database
                .settings()
                .set(
                    ACTIVE_REMOTE_PROVIDER_SETTINGS_KEY,
                    &serde_json::Value::Null,
                    OffsetDateTime::now_utc(),
                )
                .map_err(storage_error)?;
        }
    }
    remote_provider_workspace(state)
}

fn active_model_record(state: &DesktopState) -> Result<InstalledModelRecord, DesktopCommandError> {
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model database is unavailable"))?
        .models()
        .active_local_cpu()
        .map_err(storage_error)?
        .ok_or_else(|| {
            DesktopCommandError::new(
                "model_not_ready",
                "no validated local model is active; install and activate a model first",
            )
        })
}

/// Recording preferences are ordinary, non-sensitive configuration. A malformed
/// older value is reported to the UI and replaced in memory with safe defaults;
/// the next successful recording overwrites it atomically.
fn load_recording_settings(
    state: &DesktopState,
) -> Result<(RecordingSettings, Option<String>), DesktopCommandError> {
    let stored = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "settings database is unavailable"))?
        .settings()
        .get(RECORDING_SETTINGS_KEY)
        .map_err(storage_error)?;
    match stored {
        None => Ok((RecordingSettings::default(), None)),
        Some(value) => match serde_json::from_value(value) {
            Ok(settings) => Ok((settings, None)),
            Err(error) => Ok((
                RecordingSettings::default(),
                Some(format!(
                    "Saved recording preferences are invalid and were not used: {error}"
                )),
            )),
        },
    }
}

fn persist_recording_settings(
    state: &DesktopState,
    settings: &RecordingSettings,
) -> Result<(), DesktopCommandError> {
    let value = serde_json::to_value(settings).map_err(|error| {
        DesktopCommandError::new(
            "settings_serialization_failed",
            format!("recording preferences could not be saved: {error}"),
        )
    })?;
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "settings database is unavailable"))?
        .settings()
        .set(RECORDING_SETTINGS_KEY, &value, OffsetDateTime::now_utc())
        .map_err(storage_error)
}

fn load_active_lexicon_profile(
    state: &DesktopState,
) -> Result<Option<LexiconProfileId>, DesktopCommandError> {
    let stored = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "settings database is unavailable"))?
        .settings()
        .get(ACTIVE_LEXICON_PROFILE_SETTINGS_KEY)
        .map_err(storage_error)?;
    match stored {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => parse_lexicon_profile_id(&value).map(Some),
        Some(_) => Err(DesktopCommandError::new(
            "lexicon_profile_invalid",
            "the saved active lexicon profile is invalid",
        )),
    }
}

fn persist_active_lexicon_profile(
    state: &DesktopState,
    profile_id: Option<LexiconProfileId>,
) -> Result<(), DesktopCommandError> {
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "settings database is unavailable"))?;
    if let Some(profile_id) = profile_id {
        let profile = database
            .lexicon()
            .profiles()
            .map_err(storage_error)?
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                DesktopCommandError::new(
                    "lexicon_profile_not_found",
                    "lexicon profile was not found",
                )
            })?;
        if !profile.enabled {
            return Err(DesktopCommandError::new(
                "lexicon_profile_disabled",
                "a disabled lexicon profile cannot be active",
            ));
        }
    }
    let value = profile_id.map_or(serde_json::Value::Null, |id| {
        serde_json::Value::String(id.to_string())
    });
    database
        .settings()
        .set(
            ACTIVE_LEXICON_PROFILE_SETTINGS_KEY,
            &value,
            OffsetDateTime::now_utc(),
        )
        .map_err(storage_error)
}

fn load_windows_integration_settings(
    state: &DesktopState,
) -> Result<(WindowsIntegrationSettings, Option<String>), DesktopCommandError> {
    let stored = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "settings database is unavailable"))?
        .settings()
        .get(WINDOWS_INTEGRATION_SETTINGS_KEY)
        .map_err(storage_error)?;
    match stored {
        None => Ok((WindowsIntegrationSettings::default(), None)),
        Some(value) => match serde_json::from_value(value) {
            Ok(settings) => Ok((settings, None)),
            Err(error) => Ok((
                WindowsIntegrationSettings::default(),
                Some(format!(
                    "Saved Windows integration preferences are invalid and were not used: {error}"
                )),
            )),
        },
    }
}

fn persist_windows_integration_settings(
    state: &DesktopState,
    settings: &WindowsIntegrationSettings,
) -> Result<(), DesktopCommandError> {
    settings
        .hotkey
        .validate()
        .map_err(|error| DesktopCommandError::new("invalid_hotkey", error.to_string()))?;
    let value = serde_json::to_value(settings).map_err(|error| {
        DesktopCommandError::new(
            "settings_serialization_failed",
            format!("Windows integration preferences could not be saved: {error}"),
        )
    })?;
    state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "settings database is unavailable"))?
        .settings()
        .set(
            WINDOWS_INTEGRATION_SETTINGS_KEY,
            &value,
            OffsetDateTime::now_utc(),
        )
        .map_err(storage_error)
}

#[tauri::command]
fn save_windows_integration_settings(
    state: State<'_, DesktopState>,
    settings: WindowsIntegrationSettings,
) -> Result<(), DesktopCommandError> {
    // Rebinding an in-flight global key is intentionally not silent. Persisting
    // the chosen shortcut is safe; a restart registers it before recording.
    persist_windows_integration_settings(&state, &settings)
}

fn active_model_view(
    state: &DesktopState,
    manifest: Option<&free_whisper_model_manager::VerifiedManifest>,
) -> Result<Option<InstalledModelView>, DesktopCommandError> {
    let record = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "model database is unavailable"))?
        .models()
        .active_local_cpu()
        .map_err(storage_error)?;
    Ok(record.map(|record| {
        let display_name = manifest
            .and_then(|catalog| {
                catalog
                    .manifest()
                    .models
                    .iter()
                    .find(|entry| entry.id == record.model_id.as_str())
            })
            .map(|entry| entry.display_name.clone())
            .unwrap_or_else(|| record.model_id.as_str().to_owned());
        InstalledModelView {
            id: record.model_id.as_str().to_owned(),
            display_name,
            backend: record.backend,
            quantization: "F16".to_owned(),
            size_bytes: record.size_bytes,
            validated: record.validated_at.is_some(),
        }
    }))
}

fn lexicon_snapshot(
    state: &DesktopState,
) -> Result<(Vec<String>, Vec<CorrectionRule>), DesktopCommandError> {
    let active_profile = load_active_lexicon_profile(state)?;
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "lexicon database is unavailable"))?;
    let entries = database.lexicon().entries().map_err(storage_error)?;
    let prompt = select_prompt_terms(&entries, active_profile).terms;
    let mut rules = Vec::new();
    for entry in &entries {
        if !entry.scope.applies_to(active_profile) {
            continue;
        }
        for variant in database
            .lexicon()
            .variants_for(entry.id)
            .map_err(storage_error)?
        {
            rules.push(CorrectionRule {
                entry: entry.clone(),
                variant,
            });
        }
    }
    Ok((prompt, rules))
}

fn local_provider(
    app: &AppHandle,
    model: &InstalledModelRecord,
) -> Result<Arc<RecordingProvider>, DesktopCommandError> {
    let paths = locate_sidecars(app)?;
    Ok(Arc::new(RecordingProvider::Local(Arc::new(
        LocalWhisperCppProvider::new(LocalSidecarConfig {
            worker_executable: paths.worker,
            whisper_server_executable: paths.server,
            model_path: PathBuf::from(&model.installation_path),
            model_id: model.model_id.clone(),
            backend: ExecutionBackend::Cpu,
            startup_timeout: Duration::from_secs(30),
            expected_worker_version: app_version().to_owned(),
        }),
    ))))
}

async fn recording_provider(
    app: &AppHandle,
    state: &DesktopState,
) -> Result<(Arc<RecordingProvider>, ModelId), DesktopCommandError> {
    let Some(profile) = active_remote_provider(state)? else {
        let model = active_model_record(state)?;
        let model_id = model.model_id.clone();
        return Ok((local_provider(app, &model)?, model_id));
    };
    let endpoint = profile.endpoint.as_deref().ok_or_else(|| {
        DesktopCommandError::new(
            "remote_profile_invalid",
            "the active remote provider has no endpoint; configure it again",
        )
    })?;
    let credential_reference = profile.credential_reference.as_deref().ok_or_else(|| {
        DesktopCommandError::new(
            "remote_credential_missing",
            "the active remote provider has no credential reference; configure it again",
        )
    })?;
    let token = state
        .credential_store
        .read_secret(credential_reference)
        .map_err(secret_store_error)?;
    let model_id = profile
        .settings
        .remote_model_id
        .as_deref()
        .ok_or_else(|| {
            DesktopCommandError::new(
                "remote_model_not_selected",
                "select a ready model on the remote worker before recording",
            )
        })
        .and_then(|model| parse_model_id(model.to_owned()))?;
    let remote = Arc::new(
        RemoteWorkerProvider::new_with_timeouts(
            endpoint,
            &token,
            profile.settings.developer_allow_http_loopback,
            Duration::from_millis(u64::from(profile.settings.connect_timeout_ms)),
            Duration::from_millis(u64::from(profile.settings.request_timeout_ms)),
        )
        .map_err(provider_error)?,
    );
    let health = remote.health_check().await.map_err(provider_error)?;
    if !health.healthy {
        return Err(DesktopCommandError::new(
            "remote_worker_unavailable",
            "the selected remote worker reports a degraded state",
        ));
    }
    if !remote
        .refresh_capabilities()
        .await
        .map_err(provider_error)?
        .remote_execution
    {
        return Err(DesktopCommandError::new(
            "remote_capability_mismatch",
            "the selected worker does not permit remote execution",
        ));
    }
    Ok((Arc::new(RecordingProvider::Remote(remote)), model_id))
}

fn active_remote_provider(
    state: &DesktopState,
) -> Result<Option<ProviderProfileRecord>, DesktopCommandError> {
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?;
    let configured = database
        .settings()
        .get(ACTIVE_REMOTE_PROVIDER_SETTINGS_KEY)
        .map_err(storage_error)?;
    let Some(configured) = configured else {
        return Ok(None);
    };
    if configured.is_null() {
        return Ok(None);
    }
    let raw_id = configured.as_str().ok_or_else(|| {
        DesktopCommandError::new(
            "remote_profile_invalid",
            "the active remote provider setting is malformed",
        )
    })?;
    let profile_id = parse_provider_profile_id(raw_id.to_owned())?;
    let profile = database
        .providers()
        .get(profile_id)
        .map_err(storage_error)?
        .ok_or_else(|| {
            DesktopCommandError::new(
                "remote_profile_missing",
                "the active remote provider no longer exists; select another provider",
            )
        })?;
    if profile.provider_id != REMOTE_PROVIDER_ID || !profile.enabled {
        return Err(DesktopCommandError::new(
            "remote_profile_invalid",
            "the active remote provider is disabled or incompatible",
        ));
    }
    Ok(Some(profile))
}

fn active_remote_profile_id(state: &DesktopState) -> Result<Option<String>, DesktopCommandError> {
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?;
    active_remote_profile_id_from_database(&mut database)
        .map(|value| value.map(|id| id.to_string()))
}

fn active_remote_profile_id_from_database(
    database: &mut Database,
) -> Result<Option<ProviderProfileId>, DesktopCommandError> {
    let configured = database
        .settings()
        .get(ACTIVE_REMOTE_PROVIDER_SETTINGS_KEY)
        .map_err(storage_error)?;
    match configured {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => parse_provider_profile_id(value).map(Some),
        Some(_) => Err(DesktopCommandError::new(
            "remote_profile_invalid",
            "the active remote provider setting is malformed",
        )),
    }
}

fn activate_remote_profile(
    state: &DesktopState,
    profile_id: ProviderProfileId,
) -> Result<(), DesktopCommandError> {
    let mut database = state
        .database
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "provider database is unavailable"))?;
    let profile = database
        .providers()
        .get(profile_id)
        .map_err(storage_error)?
        .ok_or_else(|| {
            DesktopCommandError::new("provider_profile_missing", "provider not found")
        })?;
    if profile.provider_id != REMOTE_PROVIDER_ID
        || !profile.enabled
        || profile.endpoint.is_none()
        || profile.credential_reference.is_none()
        || profile.settings.remote_model_id.is_none()
    {
        return Err(DesktopCommandError::new(
            "remote_profile_invalid",
            "the remote provider must be enabled, have a credential and a selected model",
        ));
    }
    // Ensure that a profile never looks active when a previous external
    // credential deletion left it incomplete. The secret itself is not copied
    // into Rust state beyond this immediate validation call.
    let reference = profile.credential_reference.expect("checked above");
    state
        .credential_store
        .read_secret(&reference)
        .map_err(secret_store_error)?;
    database
        .settings()
        .set(
            ACTIVE_REMOTE_PROVIDER_SETTINGS_KEY,
            &serde_json::Value::String(profile_id.to_string()),
            OffsetDateTime::now_utc(),
        )
        .map_err(storage_error)
}

fn remote_profile_view(profile: ProviderProfileRecord) -> RemoteProviderProfileView {
    RemoteProviderProfileView {
        id: profile.id.to_string(),
        display_name: profile.display_name,
        endpoint: profile.endpoint.unwrap_or_default(),
        enabled: profile.enabled,
        credential_configured: profile.credential_reference.is_some(),
        developer_allow_http_loopback: profile.settings.developer_allow_http_loopback,
        connect_timeout_ms: profile.settings.connect_timeout_ms,
        request_timeout_ms: profile.settings.request_timeout_ms,
        selected_model_id: profile.settings.remote_model_id,
    }
}

async fn probe_remote_provider(
    input: &RemoteConnectionInput,
) -> Result<RemoteConnectionTestView, DesktopCommandError> {
    let provider = RemoteWorkerProvider::new_with_timeouts(
        input.endpoint.trim(),
        &input.bearer_token,
        input.developer_allow_http_loopback,
        Duration::from_millis(u64::from(input.connect_timeout_ms)),
        Duration::from_millis(u64::from(input.request_timeout_ms)),
    )
    .map_err(provider_error)?;
    let health = provider.health_check().await.map_err(provider_error)?;
    let capabilities = provider
        .refresh_capabilities()
        .await
        .map_err(provider_error)?;
    let models = provider
        .list_models()
        .await
        .map_err(provider_error)?
        .into_iter()
        .map(remote_model_view)
        .collect();
    Ok(RemoteConnectionTestView {
        healthy: health.healthy,
        detail: health.detail,
        remote_execution: capabilities.remote_execution,
        max_request_bytes: capabilities.max_request_bytes,
        max_audio_duration_ms: capabilities.max_audio_duration_ms,
        language_detection: capabilities.language_detection,
        word_timestamps: capabilities.word_timestamps,
        models,
    })
}

fn remote_model_view(model: AvailableModel) -> RemoteModelView {
    RemoteModelView {
        id: model.id.as_str().to_owned(),
        display_name: model.display_name,
        installed: model.installed,
        ready: model.ready,
    }
}

struct SidecarPaths {
    worker: PathBuf,
    server: PathBuf,
}

fn locate_sidecars(app: &AppHandle) -> Result<SidecarPaths, DesktopCommandError> {
    let mut roots = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries")];
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        roots.push(directory.to_path_buf());
        roots.push(directory.join("resources"));
        roots.push(directory.join("resources").join("binaries"));
    }
    if let Ok(resources) = app.path().resource_dir() {
        roots.push(resources.clone());
        roots.push(resources.join("binaries"));
    }
    let worker = first_existing(
        &roots,
        [
            "free-whisper-worker.exe",
            "free-whisper-worker-x86_64-pc-windows-msvc.exe",
        ],
    );
    let server = first_existing(&roots, ["whisper-server.exe"]);
    match (worker, server) {
        (Some(worker), Some(server)) => Ok(SidecarPaths { worker, server }),
        _ => Err(DesktopCommandError::new(
            "local_engine_unavailable",
            "the packaged local worker or whisper.cpp CPU server is missing; reinstall the application",
        )),
    }
}

fn first_existing<const N: usize>(roots: &[PathBuf], names: [&str; N]) -> Option<PathBuf> {
    roots
        .iter()
        .flat_map(|root| names.iter().map(move |name| root.join(name)))
        .find(|candidate| candidate.is_file())
}

async fn reserve_processing_slot(
    state: &DesktopState,
    job_id: JobId,
    request_id: RequestId,
    provider: Arc<RecordingProvider>,
) -> Result<(), DesktopCommandError> {
    let cancellation = CancellationToken::new();
    let mut queued = false;
    loop {
        if cancellation.is_cancelled() {
            if queued {
                clear_queued_job(state, job_id)?;
            }
            return Err(DesktopCommandError::new(
                "transcription_cancelled",
                "the queued transcription was cancelled before inference started",
            ));
        }
        // Register interest before reading processing state. That prevents a
        // release between the read and await from losing its wakeup.
        let available = state.processing_available.notified();
        let reserved = {
            let mut processing = state.processing.lock().map_err(|_| {
                DesktopCommandError::new("internal", "processing state is unavailable")
            })?;
            if processing.is_none() {
                *processing = Some(ProcessingJob {
                    job_id,
                    request_id,
                    provider: Arc::clone(&provider),
                });
                true
            } else {
                false
            }
        };
        if reserved {
            if queued {
                clear_queued_job(state, job_id)?;
            }
            return Ok(());
        }
        if !queued {
            let mut queue = state
                .queued
                .lock()
                .map_err(|_| DesktopCommandError::new("internal", "queue state is unavailable"))?;
            if queue.is_some() {
                return Err(DesktopCommandError::new(
                    "queue_full",
                    "one finalized recording is already waiting for inference",
                ));
            }
            *queue = Some(QueuedJob {
                job_id,
                cancellation: cancellation.clone(),
            });
            queued = true;
        }
        tokio::select! {
            () = available => {},
            () = cancellation.cancelled() => {},
        }
    }
}

fn clear_queued_job(state: &DesktopState, job_id: JobId) -> Result<(), DesktopCommandError> {
    let mut queue = state
        .queued
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "queue state is unavailable"))?;
    match queue.as_ref() {
        Some(queued) if queued.job_id == job_id => {
            *queue = None;
            Ok(())
        }
        Some(queued) => Err(DesktopCommandError::new(
            "job_ownership_violation",
            format!(
                "job {job_id} attempted to clear queue state owned by job {}",
                queued.job_id
            ),
        )),
        None => Err(DesktopCommandError::new(
            "queue_state_missing",
            format!("job {job_id} has no queued state to clear"),
        )),
    }
}

fn cancel_queued_job(state: &DesktopState) -> Result<Option<JobId>, DesktopCommandError> {
    let queue = state
        .queued
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "queue state is unavailable"))?;
    if let Some(queued) = queue.as_ref() {
        queued.cancellation.cancel();
        Ok(Some(queued.job_id))
    } else {
        Ok(None)
    }
}

fn clear_processing(state: &DesktopState, job_id: JobId) -> Result<(), DesktopCommandError> {
    let mut processing = state
        .processing
        .lock()
        .map_err(|_| DesktopCommandError::new("internal", "processing state is unavailable"))?;
    match processing.as_ref() {
        Some(active) if active.job_id == job_id => {
            *processing = None;
            Ok(())
        }
        Some(active) => Err(DesktopCommandError::new(
            "job_ownership_violation",
            format!(
                "job {job_id} attempted to clear processing state owned by job {}",
                active.job_id
            ),
        )),
        None => Err(DesktopCommandError::new(
            "processing_state_missing",
            format!("job {job_id} has no active processing state to clear"),
        )),
    }
}

async fn release_processing(
    state: &DesktopState,
    job_id: JobId,
    provider: &RecordingProvider,
) -> Result<(), DesktopCommandError> {
    let shutdown = provider.shutdown().await.map_err(provider_error);
    let clear = clear_processing(state, job_id);
    if clear.is_ok() {
        state.processing_available.notify_one();
    }
    match (clear, shutdown) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(clear), Ok(())) => Err(clear),
        (Ok(()), Err(shutdown)) => Err(shutdown),
        (Err(clear), Err(shutdown)) => Err(combine_cleanup_failure(clear, shutdown)),
    }
}

async fn release_processing_after_failure(
    state: &DesktopState,
    job_id: JobId,
    provider: &RecordingProvider,
    primary: DesktopCommandError,
) -> DesktopCommandError {
    match release_processing(state, job_id, provider).await {
        Ok(()) => primary,
        Err(cleanup) => combine_cleanup_failure(primary, cleanup),
    }
}

async fn shutdown_after_failure(
    provider: &RecordingProvider,
    primary: DesktopCommandError,
) -> DesktopCommandError {
    match provider.shutdown().await.map_err(provider_error) {
        Ok(()) => primary,
        Err(cleanup) => combine_cleanup_failure(primary, cleanup),
    }
}

fn combine_cleanup_failure(
    primary: DesktopCommandError,
    cleanup: DesktopCommandError,
) -> DesktopCommandError {
    DesktopCommandError::new(
        "cleanup_failed",
        format!(
            "{} The local engine cleanup also failed: {}",
            primary.message, cleanup.message
        ),
    )
}

fn language_requested(language: &LanguageSelection) -> Option<String> {
    match language {
        LanguageSelection::Auto => None,
        LanguageSelection::Explicit(language) => Some(language.clone()),
    }
}

fn transcript_view(transcript: StoredTranscript) -> TranscriptView {
    TranscriptView {
        id: transcript.id.to_string(),
        created_at: transcript.created_at.to_string(),
        raw_text: transcript.raw_text,
        final_text: transcript.final_text,
        language_requested: transcript.language_requested,
        language_detected: transcript.language_detected,
        language_confidence_milli: transcript.language_confidence_milli,
        provider_id: transcript.provider_id,
        model_id: transcript.model_id.as_str().to_owned(),
        audio_duration_ms: transcript.audio_duration_ms,
        processing_ms: transcript.processing_ms,
        queue_ms: transcript.durations.queue_ms,
        model_load_ms: transcript.durations.model_load_ms,
        inference_ms: transcript.durations.inference_ms,
        correction_ms: transcript.durations.correction_ms,
        warnings: transcript.warnings,
        corrections: transcript
            .corrections
            .into_iter()
            .map(|stored| CorrectionView {
                id: stored.correction.id.to_string(),
                original: stored.correction.original,
                replacement: stored.correction.replacement,
                start_char: stored.correction.start_char,
                end_char: stored.correction.end_char,
                reverted: stored.reverted_at.is_some(),
            })
            .collect(),
        injection_outcome: transcript.injection_outcome,
        can_paste_to_original: transcript.target_window.is_some(),
    }
}

fn emit_download_progress(app: &AppHandle, progress: ModelDownloadProgress) {
    let _ = app.emit(
        "desktop.v1.model-download-progress",
        ModelDownloadEvent {
            api_version: 1,
            model_id: progress.model_id.as_str().to_owned(),
            downloaded_bytes: progress.downloaded_bytes,
            total_bytes: progress.total_bytes,
            bytes_per_second: progress.bytes_per_second,
            estimated_remaining_seconds: progress.estimated_remaining_seconds,
        },
    );
}

fn emit_state(app: &AppHandle, job_id: JobId, state: AppState, detail: Option<String>) {
    let desktop_state = app.state::<DesktopState>();
    let sequence = desktop_state.state_sequence.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = app.emit(
        "desktop.v1.state",
        AppStateEvent {
            api_version: 1,
            sequence,
            job_id: job_id.to_string(),
            state,
            detail,
        },
    );
}

fn emit_platform_error(app: &AppHandle, area: &'static str, message: String) {
    let _ = app.emit(
        "desktop.v1.platform-error",
        PlatformErrorEvent {
            api_version: 1,
            area,
            message,
        },
    );
}

fn dispatch_hotkey_toggle(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        let capture_phase = match state.coordinator.lock() {
            Ok(coordinator) => coordinator.capture_phase(),
            Err(_) => {
                emit_platform_error(&app, "hotkey", "Capture state is unavailable.".to_owned());
                return;
            }
        };
        let result = match capture_phase {
            CapturePhase::Recording { .. } => stop_recording_inner(&app, &state).await.map(|_| ()),
            CapturePhase::Preparing { .. } | CapturePhase::Finalizing { .. } => Ok(()),
            CapturePhase::Idle => {
                let settings = load_recording_settings(&state).map(|(value, _)| value);
                match settings {
                    Ok(settings) => start_recording_inner(
                        &app,
                        &state,
                        StartRecordingInput {
                            settings,
                            language: None,
                        },
                    )
                    .await
                    .map(|_| ()),
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(error) = result {
            emit_platform_error(&app, "hotkey", error.message);
        }
    });
}

fn start_global_hotkey(app: &AppHandle) {
    let state = app.state::<DesktopState>();
    let (settings, settings_error) = match load_windows_integration_settings(&state) {
        Ok(value) => value,
        Err(error) => {
            if let Ok(mut hotkey_error) = state.hotkey_error.lock() {
                *hotkey_error = Some(error.message);
            }
            return;
        }
    };
    if let Some(error) = settings_error {
        emit_platform_error(app, "hotkey", error);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    match GlobalHotkey::start(settings.hotkey, sender) {
        Ok(hotkey) => {
            if let Ok(mut active) = state.hotkey.lock() {
                *active = Some(hotkey);
            }
            let app_handle = app.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("free-whisper-hotkey-dispatch".to_owned())
                .spawn(move || {
                    for event in receiver {
                        if matches!(event, HotkeyEvent::Pressed) {
                            dispatch_hotkey_toggle(app_handle.clone());
                        }
                    }
                })
            {
                emit_platform_error(app, "hotkey", error.to_string());
            }
        }
        Err(error) => {
            let message = error.to_string();
            if let Ok(mut hotkey_error) = state.hotkey_error.lock() {
                *hotkey_error = Some(message.clone());
            }
            emit_platform_error(app, "hotkey", message);
        }
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main")
        && let Err(error) = window.show().and_then(|_| window.set_focus())
    {
        emit_platform_error(
            app,
            "tray",
            format!("The main window could not be opened: {error}"),
        );
    }
}

fn request_clean_exit(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        let has_work = state
            .coordinator
            .lock()
            .map(|coordinator| coordinator.capture_phase() != CapturePhase::Idle)
            .unwrap_or(false)
            || state
                .processing
                .lock()
                .map(|processing| processing.is_some())
                .unwrap_or(false);
        if has_work && let Err(error) = cancel_current_job_inner(&app, &state).await {
            emit_platform_error(&app, "shutdown", error.message);
        }
        if let Ok(mut hotkey) = state.hotkey.lock()
            && let Some(hotkey) = hotkey.take()
        {
            hotkey.shutdown();
        }
        app.exit(0);
    });
}

fn configure_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Öffnen", true, None::<&str>)?;
    let toggle = MenuItem::with_id(
        app,
        "toggle-recording",
        "Aufnahme umschalten",
        true,
        None::<&str>,
    )?;
    let cancel = MenuItem::with_id(app, "cancel-job", "Abbrechen", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &toggle, &cancel, &quit])?;
    let icon = app.default_window_icon().cloned().ok_or_else(|| {
        std::io::Error::other("the configured application icon is unavailable for the tray")
    })?;
    TrayIconBuilder::with_id("free-whisper-tray")
        .icon(icon)
        .tooltip("free-whisper – lokale Transkription")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "toggle-recording" => dispatch_hotkey_toggle(app.clone()),
            "cancel-job" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<DesktopState>();
                    if let Err(error) = cancel_current_job_inner(&app, &state).await {
                        emit_platform_error(&app, "tray", error.message);
                    }
                });
            }
            "quit" => request_clean_exit(app.clone()),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn embedded_verifier() -> Result<ManifestVerifier, DesktopCommandError> {
    ManifestVerifier::from_base64(EMBEDDED_PUBLIC_KEY.trim()).map_err(|_| {
        DesktopCommandError::new(
            "model_manifest_not_provisioned",
            "the signed alpha model catalogue has not been provisioned; model downloads are unavailable",
        )
    })
}

fn verified_embedded_manifest()
-> Result<free_whisper_model_manager::VerifiedManifest, DesktopCommandError> {
    embedded_verifier()?
        .verify_for_scope(
            EMBEDDED_MANIFEST,
            EMBEDDED_SIGNATURE,
            embedded_manifest_scope()?,
        )
        .map_err(model_manager_error)
}

fn embedded_manifest_scope() -> Result<ManifestTrustScope, DesktopCommandError> {
    match env!("FREE_WHISPER_MANIFEST_SCOPE") {
        "alpha" => Ok(ManifestTrustScope::Alpha),
        "stable" => Ok(ManifestTrustScope::Stable),
        _ => Err(DesktopCommandError::new(
            "model_manifest_scope_invalid",
            "the embedded model manifest channel is invalid",
        )),
    }
}

fn parse_model_id(model_id: String) -> Result<ModelId, DesktopCommandError> {
    ModelId::parse(model_id)
        .map_err(|error| DesktopCommandError::new("invalid_model_id", error.to_string()))
}

fn parse_provider_profile_id(value: String) -> Result<ProviderProfileId, DesktopCommandError> {
    uuid::Uuid::parse_str(&value)
        .map(ProviderProfileId::from_uuid)
        .map_err(|_| {
            DesktopCommandError::new("invalid_provider_profile_id", "invalid provider profile id")
        })
}

fn parse_transcript_id(value: &str) -> Result<TranscriptId, DesktopCommandError> {
    uuid::Uuid::parse_str(value)
        .map(TranscriptId::from_uuid)
        .map_err(|_| DesktopCommandError::new("invalid_transcript_id", "invalid transcript id"))
}

fn parse_lexicon_profile_id(value: &str) -> Result<LexiconProfileId, DesktopCommandError> {
    uuid::Uuid::parse_str(value)
        .map(LexiconProfileId::from_uuid)
        .map_err(|_| {
            DesktopCommandError::new("invalid_lexicon_profile_id", "invalid lexicon profile id")
        })
}

fn parse_lexicon_entry_id(value: &str) -> Result<LexiconEntryId, DesktopCommandError> {
    uuid::Uuid::parse_str(value)
        .map(LexiconEntryId::from_uuid)
        .map_err(|_| {
            DesktopCommandError::new("invalid_lexicon_entry_id", "invalid lexicon entry id")
        })
}

fn parse_transcript_correction_id(
    value: &str,
) -> Result<TranscriptCorrectionId, DesktopCommandError> {
    uuid::Uuid::parse_str(value)
        .map(TranscriptCorrectionId::from_uuid)
        .map_err(|_| {
            DesktopCommandError::new(
                "invalid_transcript_correction_id",
                "invalid transcript correction id",
            )
        })
}

fn lexicon_csv(entries: &[LexiconEntryDraft]) -> String {
    let mut rows = vec![
        "canonical_text,language,category,priority,enabled,scope,profile_id,variant_text,match_mode"
            .to_owned(),
    ];
    for entry in entries {
        let (scope, profile_id) = match &entry.scope {
            LexiconScope::Global => ("global", String::new()),
            LexiconScope::Profile(id) => ("profile", id.to_string()),
        };
        let variants = if entry.variants.is_empty() {
            vec![None]
        } else {
            entry.variants.iter().map(Some).collect()
        };
        for variant in variants {
            let (variant_text, match_mode) =
                variant.map_or((String::new(), String::new()), |value| {
                    (
                        value.variant_text.clone(),
                        match value.match_mode {
                            MatchMode::ExactCasefold => "exact_casefold".to_owned(),
                            MatchMode::ExactCaseSensitive => "exact_case_sensitive".to_owned(),
                        },
                    )
                });
            rows.push(
                [
                    csv_field(&entry.canonical_text),
                    csv_field(entry.language.as_deref().unwrap_or_default()),
                    csv_field(entry.category.as_deref().unwrap_or_default()),
                    entry.priority.to_string(),
                    entry.enabled.to_string(),
                    scope.to_owned(),
                    csv_field(&profile_id),
                    csv_field(&variant_text),
                    match_mode,
                ]
                .join(","),
            );
        }
    }
    format!("{}\n", rows.join("\n"))
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn state_error(error: free_whisper_domain::StateTransitionError) -> DesktopCommandError {
    DesktopCommandError::new("invalid_state_transition", error.to_string())
}

fn storage_error(error: free_whisper_storage::StorageError) -> DesktopCommandError {
    DesktopCommandError::new("storage_failed", error.to_string())
}

fn secret_store_error(error: SecretStoreError) -> DesktopCommandError {
    let code = match &error {
        SecretStoreError::NotFound => "remote_credential_missing",
        SecretStoreError::InvalidReference | SecretStoreError::InvalidSecret => {
            "remote_credential_invalid"
        }
        SecretStoreError::Unavailable(_) => "credential_store_unavailable",
    };
    DesktopCommandError::new(code, error.to_string())
}

fn audio_error(error: AudioError) -> DesktopCommandError {
    let code = match error {
        AudioError::NoInputDevice | AudioError::ConfiguredDeviceUnavailable(_) => {
            "microphone_unavailable"
        }
        AudioError::DurationLimitReached => "recording_duration_limit",
        AudioError::EmptyRecording => "empty_recording",
        AudioError::BufferOverflow => "audio_buffer_overflow",
        AudioError::DeviceLost => "microphone_lost",
        _ => "audio_failed",
    };
    DesktopCommandError::new(code, error.to_string())
}

fn provider_error(error: ProviderError) -> DesktopCommandError {
    let (code, message) = match &error {
        ProviderError::ModelNotReady(_) => ("model_not_ready", error.to_string()),
        ProviderError::Cancelled(_) => ("transcription_cancelled", error.to_string()),
        ProviderError::Unavailable(_) => ("provider_unavailable", error.to_string()),
        ProviderError::SidecarOutdated => (
            "sidecar_outdated",
            "Die lokale Transkriptions-Engine passt nicht zu dieser App-Version. Starte die App über den Root-Schnellstart neu oder installiere den aktuellen Alpha.2-Installer.".to_owned(),
        ),
        ProviderError::Protocol(_) => {
            eprintln!("free-whisper diagnostic: local provider protocol failure: {error}");
            (
                "engine_protocol",
                "Die lokale Transkriptions-Engine antwortet nicht im erwarteten Format. Starte die App neu; bei erneutem Fehler exportiere eine Diagnose ohne Audio oder Text.".to_owned(),
            )
        }
        ProviderError::Rejected(_)
        | ProviderError::Unauthorized
        | ProviderError::InsecureEndpoint(_) => ("transcription_failed", error.to_string()),
    };
    DesktopCommandError::new(code, message)
}

fn model_manager_error(error: ModelManagerError) -> DesktopCommandError {
    let code = match error {
        ModelManagerError::InvalidSignature | ModelManagerError::InvalidPublicKey => {
            "model_manifest_invalid"
        }
        ModelManagerError::UnexpectedTrustScope { .. } => "model_manifest_wrong_channel",
        ModelManagerError::Cancelled => "model_download_cancelled",
        ModelManagerError::HashMismatch { .. } | ModelManagerError::SizeMismatch { .. } => {
            "model_validation_failed"
        }
        ModelManagerError::Download(_) | ModelManagerError::DownloadStatus(_) => {
            "model_download_failed"
        }
        ModelManagerError::UnknownModel(_) => "model_not_found",
        ModelManagerError::NotInstalled(_) => "model_not_ready",
        ModelManagerError::Io(_)
        | ModelManagerError::UnsafeModelStoragePath(_)
        | ModelManagerError::AmbiguousLegacyStaging(_) => "model_storage_failed",
        _ => "model_manager_failed",
    };
    let message = if code == "model_storage_failed" {
        eprintln!("free-whisper diagnostic: local model storage failure: {error}");
        "Der lokale Modellordner konnte nicht vorbereitet oder aktiviert werden. Prüfe freien Speicher und Zugriffsrechte und versuche die Installation erneut.".to_owned()
    } else {
        error.to_string()
    };
    DesktopCommandError::new(code, message)
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            let model_root = app_data.join("models");
            std::fs::create_dir_all(&model_root)?;
            let database = Database::open(app_data.join("free-whisper.sqlite3"))?;
            app.manage(DesktopState {
                model_root,
                database: Mutex::new(database),
                downloads: Mutex::new(BTreeMap::new()),
                recorder: AudioRecorder,
                windows: WindowsDesktop,
                credential_store: WindowsCredentialStore,
                hotkey: Mutex::new(None),
                hotkey_error: Mutex::new(None),
                coordinator: Mutex::new(RecordingCoordinator::default()),
                recording: Mutex::new(None),
                processing: Mutex::new(None),
                queued: Mutex::new(None),
                processing_available: Notify::new(),
                state_sequence: AtomicU64::new(0),
            });
            configure_tray(app)?;
            start_global_hotkey(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    emit_platform_error(
                        window.app_handle(),
                        "window",
                        format!("The window could not be hidden in the tray: {error}"),
                    );
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            runtime_snapshot,
            model_catalog,
            install_model,
            cancel_model_install,
            activate_model,
            delete_model,
            start_recording,
            recording_status,
            stop_recording,
            cancel_current_job,
            copy_transcript,
            paste_transcript_to_original,
            save_windows_integration_settings,
            recent_transcripts,
            search_transcripts,
            revert_transcript_correction,
            lexicon_workspace,
            save_lexicon_profile,
            delete_lexicon_profile,
            save_lexicon_entry,
            delete_lexicon_entry,
            set_active_lexicon_profile,
            import_lexicon,
            export_lexicon,
            local_provider_status,
            remote_provider_workspace,
            test_remote_provider,
            save_remote_provider,
            activate_remote_provider,
            use_local_provider,
            delete_remote_provider
        ])
        .run(tauri::generate_context!())
        .expect("Tauri runtime failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_gate_is_owned_by_the_backend_provider_state() {
        assert_eq!(onboarding_state(false), OnboardingState::SetupRequired);
        assert_eq!(onboarding_state(true), OnboardingState::Ready);
    }

    #[test]
    fn model_storage_errors_are_actionable_without_exposing_the_os_message() {
        let error = model_manager_error(ModelManagerError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "C:\\private-model-root\\missing",
        )));

        assert_eq!(error.code, "model_storage_failed");
        assert!(error.message.contains("Modellordner"));
        assert!(!error.message.contains("private-model-root"));
    }

    #[test]
    fn stale_sidecar_and_engine_protocol_failures_are_actionable_without_raw_details() {
        let stale = provider_error(ProviderError::SidecarOutdated);
        assert_eq!(stale.code, "sidecar_outdated");
        assert!(stale.message.contains("Alpha.2-Installer"));

        let protocol = provider_error(ProviderError::Protocol(
            "private inner transcript must not reach the UI".to_owned(),
        ));
        assert_eq!(protocol.code, "engine_protocol");
        assert!(!protocol.message.contains("private inner transcript"));
    }

    #[test]
    fn lexicon_csv_export_round_trips_through_the_validated_import_format() {
        let draft = LexiconEntryDraft {
            canonical_text: "free-whisper".to_owned(),
            language: Some("de".to_owned()),
            category: Some("Produkt, lokal".to_owned()),
            priority: 90,
            enabled: true,
            scope: LexiconScope::Global,
            variants: vec![LexiconVariantDraft {
                variant_text: "free whisper".to_owned(),
                match_mode: MatchMode::ExactCasefold,
            }],
        };
        let csv = lexicon_csv(&[draft]);
        let parsed = parse_lexicon_csv(&csv).expect("export is importable");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].canonical_text, "free-whisper");
        assert_eq!(parsed[0].category.as_deref(), Some("Produkt, lokal"));
        assert_eq!(parsed[0].variants.len(), 1);
    }
}
