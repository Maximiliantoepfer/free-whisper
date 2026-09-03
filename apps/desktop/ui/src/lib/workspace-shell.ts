import type { OnboardingState } from './api/desktop';

/**
 * Presentation-only mapping for the backend-owned readiness gate. Keeping it
 * here makes it explicit that the UI never guesses readiness from model or
 * provider fields.
 */
export function workspaceShellFor(state: OnboardingState | undefined): 'loading' | 'onboarding' | 'workspace' {
  if (state === undefined) return 'loading';
  return state === 'ready' ? 'workspace' : 'onboarding';
}
