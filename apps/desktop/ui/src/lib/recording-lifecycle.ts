import type { AppState } from './api/desktop';

/**
 * UI-only ordering rules. The Rust coordinator owns the actual job state; this
 * module merely prevents delayed Tauri events and pointer races from painting
 * an older state over a newer backend state.
 */
export function acceptsStateEvent(lastSequence: number, nextSequence: number): boolean {
  return Number.isSafeInteger(nextSequence) && nextSequence > lastSequence;
}

export function isTerminalState(state: AppState): boolean {
  return state === 'failed' || state === 'completed' || state === 'awaiting_injection_confirmation';
}

export type DeferredPttAction = 'none' | 'stop' | 'cancel';

export function pttActionAfterPreparation(released: boolean, cancelled: boolean): DeferredPttAction {
  if (cancelled) return 'cancel';
  return released ? 'stop' : 'none';
}
