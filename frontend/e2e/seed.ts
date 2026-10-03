import { api, apiWhenFree } from './fixtures';

/**
 * A row in every table the screens draw: two rules, an exception, a pinned
 * case, an application key, a move applied and one still proposed. The reset library has none of
 * them, and a sweep over a table with no rows checks its header and nothing a
 * row carries: a row action, a checkbox, a badge.
 *
 * The two rules differ in name on purpose: two rows called the same thing are
 * ambiguous however they are labelled, a different defect from labelling every
 * row action "Delete".
 */
export async function seedRows(): Promise<void> {
  const rules: [string, string, number, string[]][] = [
    ['Japanese animation', 'anime', 10, ['akira', 'totoro', 'perfect blue']],
    ['Science fiction', 'standard', 20, ['matrix']],
  ];
  for (const [name, category, priority, titles] of rules) {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name,
        target_category: category,
        media_type: 'movie',
        priority,
        enabled: true,
        condition_logic: 'any',
        conditions: [{ type: 'title_contains', value: titles }],
        exclusions: [],
      }),
    });
  }

  const { data: films } = (await api('/media')) as { data: { id: string; title: string }[] };
  const film = (title: string) => {
    const found = films.find((item) => item.title === title);
    if (!found) throw new Error(`${title} is not in the library`);
    return found.id;
  };
  await api('/overrides', {
    method: 'POST',
    body: JSON.stringify({ media_id: film('My Neighbor Totoro'), target_category: 'standard' }),
  });
  await api('/rule-tests', {
    method: 'POST',
    body: JSON.stringify({ name: 'Akira is anime', media_id: film('Akira') }),
  });
  await api('/applications', {
    method: 'POST',
    body: JSON.stringify({ name: 'Home Assistant', scopes: ['operate'], may_confirm: ['batch'] }),
  });

  const decisions = await simulate();

  // History keeps every move ever applied and no reset clears it, so a move is
  // applied once in a run rather than once per test: Akira moved in every test
  // would fill the table with rows no installation holds, and every sweep would
  // walk them.
  const { data: applied } = (await api('/decisions?status=applied&per_page=200')) as {
    data: { reverted_at: string | null }[];
  };
  if (applied.some((decision) => !decision.reverted_at)) return;
  await moveAkira(decisions);
}

export type Proposal = { id: string; media_title: string; action: string };

export async function simulate(): Promise<Proposal[]> {
  const { decisions } = (await apiWhenFree('/simulate', {
    method: 'POST',
    body: JSON.stringify({ persist: true }),
  })) as { decisions: Proposal[] };
  return decisions;
}

export async function moveAkira(decisions: Proposal[]): Promise<void> {
  const akira = decisions.find((d) => d.media_title === 'Akira' && d.action === 'move');
  if (!akira) throw new Error('the simulation proposes no move for Akira');
  // Nothing is written while the global dry run holds, and it holds again for
  // the sweeps, the shipped posture.
  const dryRun = (on: boolean) =>
    api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { global_dry_run: String(on) } }),
    });
  await dryRun(false);
  await api('/decisions/apply', {
    method: 'POST',
    body: JSON.stringify({ decision_ids: [akira.id], move_files: false, confirm: [] }),
  });
  await dryRun(true);
}
