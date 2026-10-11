import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const ROOT = resolve(__dirname, '..', '..', '..');

/**
 * The backend files whose change wakes the frontend job in CI, as
 * `scripts/changed-areas.sh` writes them. A test reading a file this misses
 * runs on no pull request that changes that file alone, and the next one to
 * touch the frontend inherits the failure.
 */
const WAKES_THE_FRONTEND: RegExp = (() => {
  const script = readFileSync(join(ROOT, 'scripts', 'changed-areas.sh'), 'utf8');
  const reads = /^FRONTEND_READS='([^']+)'$/m.exec(script)?.[1];
  if (reads === undefined) throw new Error('scripts/changed-areas.sh sets no FRONTEND_READS');
  return new RegExp(`^(${reads})$`);
})();

/** A file of the backend, by its path from the repository root, as text. */
export function readBackend(file: string): string {
  if (!WAKES_THE_FRONTEND.test(file)) {
    throw new Error(
      `${file} is read by a frontend test, and FRONTEND_READS in scripts/changed-areas.sh ` +
        'does not wake the frontend job when it changes',
    );
  }
  return readFileSync(join(ROOT, file), 'utf8');
}
