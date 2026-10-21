import { describe, it, expect } from 'vitest';

import { access, publishAccess } from './access.svelte';

/**
 * What the shell learnt of the person signed in. Until it has asked, nobody is taken for a
 * reader: a screen rendered before the answer shows what the owner sees, and the server refuses
 * a reader whatever the screen offered.
 */

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
