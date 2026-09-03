import { describe, expect, it } from 'vitest';
import {
  acceptsStateEvent,
  isTerminalState,
  noSpeechNotice,
  pttActionAfterPreparation
} from './recording-lifecycle';

describe('recording UI lifecycle', () => {
  it('rejects delayed or invalid state events', () => {
    expect(acceptsStateEvent(8, 9)).toBe(true);
    expect(acceptsStateEvent(8, 8)).toBe(false);
    expect(acceptsStateEvent(8, 7)).toBe(false);
    expect(acceptsStateEvent(8, Number.NaN)).toBe(false);
  });

  it('settles a fast push-to-talk release after preparation', () => {
    expect(pttActionAfterPreparation(true, false)).toBe('stop');
    expect(pttActionAfterPreparation(true, true)).toBe('cancel');
    expect(pttActionAfterPreparation(false, false)).toBe('none');
  });

  it('stops polling for every terminal result state', () => {
    expect(isTerminalState('failed')).toBe(true);
    expect(isTerminalState('completed')).toBe(true);
    expect(isTerminalState('awaiting_injection_confirmation')).toBe(true);
    expect(isTerminalState('finalizing_audio')).toBe(false);
  });

  it('presents a short or silent capture as a neutral no-op', () => {
    expect(noSpeechNotice()).toBe('Keine Sprache aufgenommen – es wurde nichts transkribiert.');
  });
});
