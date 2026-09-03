export const appStates = [
  'idle',
  'preparing_recording',
  'recording',
  'finalizing_audio',
  'queued',
  'loading_model',
  'transcribing',
  'applying_corrections',
  'awaiting_injection_confirmation',
  'injecting',
  'completed',
  'failed',
  'cancelling'
] as const;

export type AppState = (typeof appStates)[number];

export function labelForState(state: AppState): string {
  const labels: Record<AppState, string> = {
    idle: 'Bereit',
    preparing_recording: 'Aufnahme wird vorbereitet',
    recording: 'Aufnahme läuft',
    finalizing_audio: 'Audio wird abgeschlossen',
    queued: 'Wartet auf Verarbeitung',
    loading_model: 'Modell wird geladen',
    transcribing: 'Wird transkribiert',
    applying_corrections: 'Korrekturen werden angewendet',
    awaiting_injection_confirmation: 'Text wartet auf Bestätigung',
    injecting: 'Text wird eingefügt',
    completed: 'Abgeschlossen',
    failed: 'Fehler',
    cancelling: 'Wird abgebrochen'
  };

  return labels[state];
}
