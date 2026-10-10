import { api, apiWhenFree, ARR } from './fixtures';

/**
 * A row in every table the screens draw: three rules, one of them switched
 * off, a second instance switched off, an exception, a pinned case, an
 * application key, a move applied and one still proposed. The reset library
 * has none of them, and a sweep over a table with no rows checks its header
 * and nothing a row carries: a row action, a checkbox, a badge, the colours
 * of a row switched off.
 *
 * The two rules differ in name on purpose: two rows called the same thing are
 * ambiguous however they are labelled, a different defect from labelling every
 * row action "Delete".
 */
export async function seedRows(): Promise<void> {
  const rules: [string, string, number, string[], boolean][] = [
    ['Japanese animation', 'anime', 10, ['akira', 'totoro', 'perfect blue'], true],
    ['Science fiction', 'standard', 20, ['matrix'], true],
    ['Westerns', 'standard', 30, ['django'], false],
  ];
  for (const [name, category, priority, titles, enabled] of rules) {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name,
        target_category: category,
        media_type: 'movie',
        priority,
        enabled,
        conditions: [{ type: 'title_contains', value: titles }],
        exclusions: [],
      }),
    });
  }

  await api('/instances', {
    method: 'POST',
    body: JSON.stringify({
      name: 'Radarr 4K',
      instance_type: 'radarr',
      base_url: ARR,
      api_key: 'e2e',
      enabled: false,
      sync_interval_minutes: 15,
    }),
  });

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
