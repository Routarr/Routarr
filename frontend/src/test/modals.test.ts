import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

const SRC = path.resolve(__dirname, '..');
const SWEEP = path.resolve(SRC, '..', 'e2e', 'accessibility.spec.ts');

/**
 * Every `<Modal>` a file renders needs its own entry in the sweep's `MODALS` whose `covers:`
 * names that file, since a dialog nobody adds there is never opened.
 */
function dialogs(): string[] {
  return ['pages', 'components'].flatMap((dir) =>
    fs
      .readdirSync(path.join(SRC, dir))
      .filter((f) => f.endsWith('.svelte'))
      // `Modal.svelte` is the dialog itself, not a use of it.
      .filter((f) => f !== 'Modal.svelte')
      .flatMap((f) =>
        (fs.readFileSync(path.join(SRC, dir, f), 'utf-8').match(/<Modal\b/g) ?? []).map(
          () => `${dir}/${f}`,
        ),
      ),
  );
}

/** How many times each item appears in a list. */
function tally(items: string[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const item of items) counts.set(item, (counts.get(item) ?? 0) + 1);
  return counts;
}

/**
 * The dialog `Layout` always mounts, which both searches must find: compared with an empty
 * list, each check below passes, as it would if the import were aliased or `covers:` renamed.
 */
const ANCHOR = 'components/ConfirmDialog.svelte';

describe('the modal accessibility sweep', () => {
  it('opens every dialog the application can render', () => {
    const sweep = fs.readFileSync(SWEEP, 'utf-8');
    const covered = tally([...sweep.matchAll(/covers: '([^']+)'/g)].map((match) => match[1]!));

    const rendered = dialogs();
    expect(rendered, 'the search for files rendering <Modal> found nothing').toContain(ANCHOR);

    const uncovered = [...tally(rendered)]
      .filter(([file, dialogs]) => (covered.get(file) ?? 0) < dialogs)
      .map(([file, dialogs]) => `${file}: ${dialogs} dialogs, ${covered.get(file) ?? 0} opened`);
    expect(uncovered).toEqual([]);
  });

  it('names only files that exist', () => {
    const sweep = fs.readFileSync(SWEEP, 'utf-8');
    const named = [...sweep.matchAll(/covers: '([^']+)'/g)]
      .map((match) => match[1])
      .filter((file) => file !== undefined);
    expect(named, 'the sweep declares no `covers:` at all').toContain(ANCHOR);

    const missing = named.filter((file) => !fs.existsSync(path.join(SRC, file)));
    expect(missing).toEqual([]);
  });
});
