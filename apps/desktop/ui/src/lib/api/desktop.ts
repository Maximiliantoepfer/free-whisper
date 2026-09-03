import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type AppState =
  | 'idle'
  | 'preparing_recording'
  | 'recording'
  | 'finalizing_audio'
  | 'queued'
  | 'loading_model'
  | 'transcribing'
  | 'applying_corrections'
  | 'awaiting_injection_confirmation'
  | 'injecting'
  | 'completed'
  | 'failed'
  | 'cancelling';

export type CapturePhase = 'idle' | 'preparing' | 'recording' | 'finalizing';

export interface DesktopError {
  code: string;
  message: string;
}

export interface InputDevice {
  id: string;
  name: string;
  isDefault: boolean;
  sampleRateHz: number;
  channels: number;
}

export interface RecordingStatus {
  durationMs: number;
  peakMilli: number;
  maxDurationReached: boolean;
  vadEndDetected: boolean;
}

export interface VadSettings {
  enabled: boolean;
  aggressiveness: number;
  minimumSpeechMs: number;
  endSilenceMs: number;
}

export interface RecordingSettings {
  deviceId: string | null;
  mode: 'push_to_talk' | 'toggle';
  maxDurationSeconds: number;
  vad: VadSettings;
}

export interface HotkeyBinding {
  control: boolean;
  alt: boolean;
  shift: boolean;
  key: 'space';
}

export interface WindowsIntegrationSettings {
  hotkey: HotkeyBinding;
  autoPaste: boolean;
  restoreClipboardAfterPaste: boolean;
}

export interface InstalledModel {
  id: string;
  displayName: string;
  backend: string;
  quantization: string;
  sizeBytes: number;
  validated: boolean;
}

export interface ActiveProvider {
  kind: 'local' | 'remote';
  displayName: string;
  modelId: string;
  backend: string;
}

export type OnboardingState = 'setup_required' | 'ready';

export interface ModelCatalogItem {
  id: string;
  displayName: string;
  preset: 'fast' | 'balanced' | 'high_quality';
  sizeBytes: number;
  backend: string;
  quantization: string;
  estimatedRamBytes: number;
  license: string;
  licenseUrl: string;
  sourceUrl: string;
  installed: boolean;
  active: boolean;
  validated: boolean;
}

export interface Correction {
  id: string;
  original: string;
  replacement: string;
  startChar: number;
  endChar: number;
  reverted: boolean;
}

export type InjectionOutcome =
  | 'inserted'
  | 'copied_to_clipboard'
  | { needs_user_confirmation: { reason: string } }
  | { failed_with_reason: { reason: string } };

export interface Transcript {
  id: string;
  createdAt: string;
  rawText: string;
  finalText: string;
  languageRequested: string | null;
  languageDetected: string | null;
  languageConfidenceMilli: number | null;
  providerId: string;
  modelId: string;
  audioDurationMs: number;
  processingMs: number;
  queueMs: number;
  modelLoadMs: number;
  inferenceMs: number;
  correctionMs: number;
  warnings: string[];
  corrections: Correction[];
  injectionOutcome: InjectionOutcome;
  canPasteToOriginal: boolean;
}

export interface RuntimeSnapshot {
  appVersion: string;
  /** Backend-owned gate: no workspace navigation is rendered before a provider is active. */
  onboarding: OnboardingState;
  manifestAvailable: boolean;
  manifestError: string | null;
  activeModel: InstalledModel | null;
  activeProvider: ActiveProvider | null;
  inputDevices: InputDevice[];
  microphoneError: string | null;
  recordingSettings: RecordingSettings;
  recordingSettingsError: string | null;
  windowsIntegration: WindowsIntegrationSettings;
  windowsIntegrationError: string | null;
  hotkeyError: string | null;
  capturePhase: CapturePhase;
  activeJobId: string | null;
  recording: RecordingStatus | null;
  processing: boolean;
  queueDepth: number;
  activeLexiconProfileId: string | null;
  lastTranscript: Transcript | null;
}

export type MatchMode = 'exact_casefold' | 'exact_case_sensitive';

export interface LexiconProfile {
  id: string;
  name: string;
  enabled: boolean;
}

export interface LexiconVariant {
  id: string;
  variantText: string;
  matchMode: MatchMode;
}

export interface LexiconEntry {
  id: string;
  canonicalText: string;
  language: string | null;
  category: string | null;
  priority: number;
  enabled: boolean;
  profileId: string | null;
  variants: LexiconVariant[];
}

export interface LexiconWorkspace {
  activeProfileId: string | null;
  profiles: LexiconProfile[];
  entries: LexiconEntry[];
}

export interface LexiconImportReport {
  addedEntries: number;
  addedVariants: number;
  duplicateEntries: number;
  duplicateVariants: number;
  skipped: number;
}

export interface LexiconExport {
  format: 'csv' | 'json';
  content: string;
}

export interface LocalProviderStatus {
  providerId: string;
  workerAvailable: boolean;
  whisperServerAvailable: boolean;
  activeModelId: string | null;
  activeModelValidated: boolean;
  backend: 'cpu';
  detail: string;
}

export interface RemoteProviderProfile {
  id: string;
  displayName: string;
  endpoint: string;
  enabled: boolean;
  credentialConfigured: boolean;
  developerAllowHttpLoopback: boolean;
  connectTimeoutMs: number;
  requestTimeoutMs: number;
  selectedModelId: string | null;
}

export interface RemoteProviderWorkspace {
  activeProfileId: string | null;
  profiles: RemoteProviderProfile[];
}

export interface RemoteModel {
  id: string;
  displayName: string;
  installed: boolean;
  ready: boolean;
}

export interface RemoteConnectionTest {
  healthy: boolean;
  detail: string;
  remoteExecution: boolean;
  maxRequestBytes: number;
  maxAudioDurationMs: number;
  languageDetection: boolean;
  wordTimestamps: boolean;
  models: RemoteModel[];
}

export interface RemoteConnectionInput {
  endpoint: string;
  bearerToken: string;
  developerAllowHttpLoopback: boolean;
  connectTimeoutMs: number;
  requestTimeoutMs: number;
}

export interface DownloadProgress {
  apiVersion: 1;
  modelId: string;
  downloadedBytes: number;
  totalBytes: number;
  bytesPerSecond: number;
  estimatedRemainingSeconds: number | null;
}

export interface StateChange {
  apiVersion: 1;
  sequence: number;
  jobId: string;
  state: AppState;
  detail: string | null;
}

export interface PlatformError {
  apiVersion: 1;
  area: 'hotkey' | 'target_window' | 'clipboard_restore' | 'tray' | 'shutdown' | 'window';
  message: string;
}

export function desktopError(error: unknown): DesktopError {
  if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error) {
    const candidate = error as { code?: unknown; message?: unknown };
    if (typeof candidate.code === 'string' && typeof candidate.message === 'string') {
      return { code: candidate.code, message: candidate.message };
    }
  }
  return {
    code: 'unexpected_error',
    message: typeof error === 'string' ? error : 'An unexpected desktop error occurred.'
  };
}

export const desktop = {
  appVersion: () => invoke<string>('app_version'),
  snapshot: () => invoke<RuntimeSnapshot>('runtime_snapshot'),
  modelCatalog: () => invoke<ModelCatalogItem[]>('model_catalog'),
  installModel: (modelId: string) => invoke<void>('install_model', { modelId }),
  cancelInstall: (modelId: string) => invoke<void>('cancel_model_install', { modelId }),
  activateModel: (modelId: string) => invoke<void>('activate_model', { modelId }),
  deleteModel: (modelId: string, confirmedByUser: boolean) =>
    invoke<void>('delete_model', { modelId, confirmedByUser }),
  startRecording: (input: { settings: RecordingSettings; language: string | null }) =>
    invoke<RecordingStatus>('start_recording', { input }),
  recordingStatus: () => invoke<RecordingStatus | null>('recording_status'),
  stopRecording: () => invoke<Transcript>('stop_recording'),
  cancelCurrentJob: () => invoke<void>('cancel_current_job'),
  copyTranscript: (transcriptId: string) => invoke<void>('copy_transcript', { transcriptId }),
  pasteTranscriptToOriginal: (transcriptId: string) =>
    invoke<Transcript>('paste_transcript_to_original', { transcriptId }),
  saveWindowsIntegrationSettings: (settings: WindowsIntegrationSettings) =>
    invoke<void>('save_windows_integration_settings', { settings }),
  recentTranscripts: () => invoke<Transcript[]>('recent_transcripts'),
  searchTranscripts: (query: string) => invoke<Transcript[]>('search_transcripts', { query }),
  revertTranscriptCorrection: (transcriptId: string, correctionId: string) =>
    invoke<Transcript>('revert_transcript_correction', { transcriptId, correctionId }),
  lexiconWorkspace: () => invoke<LexiconWorkspace>('lexicon_workspace'),
  saveLexiconProfile: (input: { id: string | null; name: string; enabled: boolean }) =>
    invoke<LexiconWorkspace>('save_lexicon_profile', { input }),
  deleteLexiconProfile: (profileId: string) =>
    invoke<LexiconWorkspace>('delete_lexicon_profile', { profileId }),
  saveLexiconEntry: (input: {
    canonicalText: string;
    language: string | null;
    category: string | null;
    priority: number;
    enabled: boolean;
    profileId: string | null;
    variants: Array<{ variantText: string; matchMode: MatchMode }>;
  }) => invoke<LexiconWorkspace>('save_lexicon_entry', { input }),
  deleteLexiconEntry: (entryId: string) =>
    invoke<LexiconWorkspace>('delete_lexicon_entry', { entryId }),
  setActiveLexiconProfile: (profileId: string | null) =>
    invoke<LexiconWorkspace>('set_active_lexicon_profile', { profileId }),
  importLexicon: (format: 'csv' | 'json', content: string) =>
    invoke<LexiconImportReport>('import_lexicon', { input: { format, content } }),
  exportLexicon: (format: 'csv' | 'json') =>
    invoke<LexiconExport>('export_lexicon', { format }),
  localProviderStatus: () => invoke<LocalProviderStatus>('local_provider_status'),
  remoteProviderWorkspace: () => invoke<RemoteProviderWorkspace>('remote_provider_workspace'),
  testRemoteProvider: (input: RemoteConnectionInput) =>
    invoke<RemoteConnectionTest>('test_remote_provider', { input }),
  saveRemoteProvider: (input: RemoteConnectionInput & {
    id: string | null;
    displayName: string;
    selectedModelId: string;
    enabled: boolean;
    activateAfterSave: boolean;
  }) => invoke<RemoteProviderWorkspace>('save_remote_provider', { input }),
  activateRemoteProvider: (profileId: string) =>
    invoke<RemoteProviderWorkspace>('activate_remote_provider', { profileId }),
  useLocalProvider: () => invoke<RemoteProviderWorkspace>('use_local_provider'),
  deleteRemoteProvider: (profileId: string, confirmedByUser: boolean) =>
    invoke<RemoteProviderWorkspace>('delete_remote_provider', { profileId, confirmedByUser })
};

export function onDownloadProgress(handler: (progress: DownloadProgress) => void): Promise<UnlistenFn> {
  return listen<DownloadProgress>('desktop.v1.model-download-progress', (event) => handler(event.payload));
}

export function onStateChange(handler: (event: StateChange) => void): Promise<UnlistenFn> {
  return listen<StateChange>('desktop.v1.state', (event) => handler(event.payload));
}

export function onPlatformError(handler: (event: PlatformError) => void): Promise<UnlistenFn> {
  return listen<PlatformError>('desktop.v1.platform-error', (event) => handler(event.payload));
}
