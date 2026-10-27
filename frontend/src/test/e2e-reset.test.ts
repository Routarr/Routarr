import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

const E2E = path.resolve(__dirname, '..', '..', 'e2e');

/** The keys of each `settings: { ... }` object a file of the suite writes. */
function settingsWritten(file: string): string[] {
  const text = fs.readFileSync(path.join(E2E, file), 'utf-8').replace(/^\s*\/\/.*$/gm, '');
  return [...text.matchAll(/settings:\s*\{([^}]*)\}/g)].flatMap((written) =>
    [...written[1]!.matchAll(/(?:^|[{,])\s*(\w+)\s*:/g)].map((key) => key[1]!),
  );
}

describe('the reset before every end-to-end test', () => {
  /**
   * The suite runs serially against one server, so a setting a spec writes and the reset leaves
   * decides the posture of every spec after it, and of none when that spec runs alone.
   */
  it('writes back every setting a spec writes', () => {
    const reset = new Set(settingsWritten('fixtures.ts'));
    expect([...reset], 'the reset writes no settings').toContain('ui_language');

    const specs = fs
      .readdirSync(E2E)
      .filter((file) => file.endsWith('.ts') && file !== 'fixtures.ts');
    const written = specs.flatMap((file) => settingsWritten(file).map((key) => ({ file, key })));
    expect(written.length, 'no spec writes a setting').toBeGreaterThan(0);

    const left = written
      .filter(({ key }) => !reset.has(key))
      .map(({ file, key }) => `${file}: ${key}`);
    expect(left).toEqual([]);
  });
});
