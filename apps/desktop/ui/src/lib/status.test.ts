import { describe, expect, it } from 'vitest';

import { appStates, labelForState } from './status';

describe('labelForState', () => {
  it('has an accessible label for every backend state', () => {
    for (const state of appStates) {
      expect(labelForState(state)).not.toHaveLength(0);
    }
  });
});
