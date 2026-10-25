import { describe, it, expect } from 'vitest';

import { readBackend } from '../test/backend';
import { SERVER_COUNTS } from '../test/counts';
import { ENGLISH } from '../test/render';
import { warning } from '../test/fixtures';
import { seedDictionary } from './i18n.svelte';
import { SCREENS } from './routes';
import { SETTLED_ON, fixOf } from './warnings';

/** The codes `CODES` lists in `backend/src/api/health.rs`. */
function serverCodes(): string[] {
  const source = readBackend('backend/src/api/health.rs');
  const start = source.indexOf('pub(crate) const CODES');
  const table = source.slice(start, source.indexOf('];', start));
  return [...table.matchAll(/\("([a-z_]+)", "[a-z]+"\)/g)].map((match) => match[1]!);
}

describe('where a warning is settled', () => {
  /** A code added later with no entry would get no link, and nothing else would say so. */
  it('names a screen, or none, for every code the server emits and for nothing else', () => {
    const codes = serverCodes();
    expect(codes, 'no code was read from health.rs').toContain('rule_tests_failing');

    expect(Object.keys(SETTLED_ON).sort()).toEqual([...codes].sort());
  });

  it('sends every warning to a screen that exists', () => {
    const screens = Object.values(SETTLED_ON).filter((to) => to !== null);
    expect(screens.length).toBeGreaterThan(0);
    for (const to of screens) expect(SCREENS).toContain(to.split('#')[0]);
  });

  it('names the link after its screen, and offers none where no screen settles it', () => {
    seedDictionary(ENGLISH, 'en', SERVER_COUNTS);

    expect(fixOf(warning('x', null, 'rule_tests_failing'))).toEqual({
      to: '/rule-tests',
      label: 'Open Rule tests',
    });
    expect(fixOf(warning('x', null, 'backup_copy_failed'))?.label).toBe('Open Settings');
    expect(fixOf(warning('x', null, 'data_disk_low'))).toBeNull();
    expect(fixOf(warning('x', null, 'a_code_of_a_later_release'))).toBeNull();
  });
});
