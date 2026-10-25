import { describe, it, expect } from 'vitest';

import { access, publishAccess } from './access.svelte';

describe('access', () => {
  it('is unknown, and no reader, until the shell has asked', () => {
    expect(access.known).toBe(false);
    expect(access.readOnly).toBe(false);
  });

  it.each([true, false])('holds what the shell was told, read only %s', (readOnly) => {
    publishAccess(readOnly);

    expect(access.known).toBe(true);
    expect(access.readOnly).toBe(readOnly);
  });

  it('forgets the answer when told to', () => {
    publishAccess(true);
    publishAccess(null);

    expect(access.known).toBe(false);
    expect(access.readOnly).toBe(false);
  });
});
