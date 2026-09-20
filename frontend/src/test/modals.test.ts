import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

const SRC = path.resolve(__dirname, '..');
const SWEEP = path.resolve(SRC, '..', 'e2e', 'accessibility.spec.ts');

/**
 * The accessibility sweep over the modals keeps its list by hand, and a list
 * kept by hand is a list that stops being complete.
 *
 * It is the same shape as selecting e2e specs by filename, where a new spec is
 * silently never run: an eighth dialog added to `MODALS` by nobody is a dialog
 * nothing opens, and nothing would say so.
 *
 * So the source is the authority: every file that renders a `<Modal>` has to be
 * named in a `covers:` field. Writing a modal and not sweeping it fails here,
 * in a second, rather than never.
 */
function modalBearingFiles(): string[] {
  return ['pages', 'components'].flatMap((dir) =>
    fs
      .readdirSync(path.join(SRC, dir))
      .filter((f) => f.endsWith('.svelte'))
      .filter((f) => fs.readFileSync(path.join(SRC, dir, f), 'utf-8').includes('<Modal'))
      // `Modal.svelte` is the dialog itself, not a use of it.
      .filter((f) => f !== 'Modal.svelte')
      .map((f) => `${dir}/${f}`),
  );
}

/**
 * Both checks below compare two lists, and a comparison against an empty list
 * passes. Neither search is safe on its own: `<Modal` stops matching if the
 * import is ever aliased, and `covers:` stops matching if the sweep renames its
 * field — and then the check that guarantees the sweep is complete reports
 * nothing rather than everything. Removing the `<Modal` match was tried, and
 * both tests went green over a search that found no file at all.
 */
const ANCHOR = 'components/ConfirmDialog.svelte';

describe('the modal accessibility sweep', () => {
  it('opens every dialog the application can render', () => {
    const sweep = fs.readFileSync(SWEEP, 'utf-8');
    const covered = new Set([...sweep.matchAll(/covers: '([^']+)'/g)].map((match) => match[1]));

    const bearing = modalBearingFiles();
    // `ConfirmDialog` is mounted by `Layout` and is the one dialog the
    // application always has, so a search that misses it found nothing.
    expect(bearing, 'the search for files rendering <Modal> found nothing').toContain(ANCHOR);

    const uncovered = bearing.filter((file) => !covered.has(file));
    expect(uncovered).toEqual([]);
  });

  it('names only files that exist', () => {
    // The other direction: a component renamed or deleted leaves a `covers:`
    // pointing at nothing, and the sweep then proves less than it claims.
    const sweep = fs.readFileSync(SWEEP, 'utf-8');
    const named = [...sweep.matchAll(/covers: '([^']+)'/g)]
      .map((match) => match[1])
      .filter((file) => file !== undefined);
    // Nothing named is not nothing wrong: the field was renamed, and this
    // check then reads an empty list and reports it clean.
    expect(named, 'the sweep declares no `covers:` at all').toContain(ANCHOR);

    const missing = named.filter((file) => !fs.existsSync(path.join(SRC, file)));
    expect(missing).toEqual([]);
  });
});
