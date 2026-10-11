import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

import { readBackend } from './backend';

const SRC = join(__dirname, '..');

/** Every TypeScript file under `src/`, by its path from there. */
function sources(dir = SRC): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    return entry.name.endsWith('.ts') ? [path.slice(SRC.length + 1)] : [];
  });
}

describe('readBackend', () => {
  /**
   * The one door, so the CI pattern it checks cannot be walked around: a path
   * written by hand is a file the frontend job may not run for.
   */
  it('is the one way a frontend test reads a backend file', () => {
    const files = sources();
    expect(files.length).toBeGreaterThan(100);
    const byHand = files.filter((file) =>
      /['"](\.\.\/)+backend\/|['"]backend['"]\s*,/.test(readFileSync(join(SRC, file), 'utf8')),
    );
    expect(byHand).toEqual([]);
  });

  it('refuses a file a change to which does not wake the frontend job', () => {
    expect(() => readBackend('backend/src/api/rules.rs')).toThrow(/FRONTEND_READS/);
    expect(readBackend('backend/locales/en.json')).toContain('"Dashboard"');
  });
});
