import { describe, expect, it } from 'vitest';

import { workspaceShellFor } from './workspace-shell';

describe('workspaceShellFor', () => {
  it('keeps workspace navigation unavailable until the backend marks setup ready', () => {
    expect(workspaceShellFor(undefined)).toBe('loading');
    expect(workspaceShellFor('setup_required')).toBe('onboarding');
    expect(workspaceShellFor('ready')).toBe('workspace');
  });
});
