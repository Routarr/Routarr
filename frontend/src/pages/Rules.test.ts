import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api, ApiError } from '../api/client';
import type { Category, ConditionCatalog, Rule } from '../api/types';
import Rules from './Rules.svelte';
import { answerConfirmation } from '../test/confirm';

/**
 * First match by ascending priority wins, so the order of this table *is* the
 * routing. Reordering it rewrites priorities on the server; nothing about it is
 * cosmetic.
 */

const STRINGS = {
  RulesEngine: 'Rules',
  NewRule: 'New rule',
  NoRulesYet: 'No rule yet',
  RaisePriority: 'Raise priority',
  LowerPriority: 'Lower priority',
  Edit: 'Edit',
  Duplicate: 'Duplicate',
  Delete: 'Delete',
  Disabled: 'disabled',
  PrioritiesUpdated: 'Priorities updated',
  RuleDuplicated: 'Rule duplicated',
  RuleDeleted: 'Rule deleted',
  ConfirmDeleteRule: 'Delete the rule "{name}"?',
  LogicAll: 'ALL',
  LogicAny: 'ANY',
  Both: 'Movies and series',
  MoviesOnly: 'Movies only',
  SeriesOnly: 'Series only',
  ExceptPrefix: 'except',
  AddCondition: 'Add a condition',
  ImportResult: 'Rules imported: {count}',
  ImportSkipped: ', skipped: {count}',
  ImportReplaceQuestion: 'Replace the rules, or add to them?',
  ImportAppend: 'Add',
  ImportReplace: 'Replace',
};

const catalog: ConditionCatalog = {
  match_modes: ['all', 'any'],
  conditions: [
    {
      type: 'genre_contains',
      label: 'Genre contains',
      value_type: 'string_list',
      needs_metadata: true,
      metadata_field: 'genres',
      suggestions: 'genres',
      quantifier: '',
      counterpart: '',
      media_types: ['movie', 'series'],
      available: true,
    },
  ],
};

const categories = [
  {
    id: 'c1',
    name: 'anime',
    description: null,
    is_default: false,
    display_order: 1,
    created_at: '2026-08-27 10:00:00',
    rule_count: 1,
    root_folder_count: 1,
  },
] as Category[];

function rule(over: Partial<Rule> = {}): Rule {
  return {
    id: 'r1',
    name: 'Japanese animation',
    description: null,
    priority: 10,
    enabled: true,
    media_type: 'both',
    conditions: [{ type: 'genre_contains', value: ['Animation'] }],
    exclusions: [],
    match_mode: 'all',
    target_category: 'anime',
    instance_ids: null,
    created_at: '2026-08-27 10:00:00',
    updated_at: '2026-08-27 10:00:00',
    ...over,
  };
}

function show(rules: Rule[]) {
  vi.spyOn(api, 'getRules').mockResolvedValue(rules);
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  vi.spyOn(api, 'getConditionCatalog').mockResolvedValue(catalog);
  return renderWithI18n(Rules, { strings: STRINGS });
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Rules', () => {
  it('invites a first rule instead of showing an empty table', async () => {
    show([]);

    expect(await screen.findByText('No rule yet')).toBeTruthy();
  });

  it('names each row action after its rule', async () => {
    show([rule({ name: 'Japanese animation' }), rule({ id: 'r2', name: 'Kids' })]);

    await screen.findByText('Japanese animation');
    for (const name of ['Edit – Kids', 'Duplicate – Kids', 'Delete – Kids']) {
      expect(screen.getByRole('button', { name })).toBeTruthy();
    }
  });

  /**
   * The first rule cannot be raised and the last cannot be lowered. Offering
   * either sends the server a reorder that changes nothing and reports success.
   */
  it('cannot raise the first rule or lower the last', async () => {
    show([rule({ id: 'r1', name: 'First' }), rule({ id: 'r2', name: 'Last' })]);

    await screen.findByText('First');
    const raiseFirst = screen.getByRole('button', { name: 'Raise priority – First' });
    const lowerLast = screen.getByRole('button', { name: 'Lower priority – Last' });

    expect((raiseFirst as HTMLButtonElement).disabled).toBe(true);
    expect((lowerLast as HTMLButtonElement).disabled).toBe(true);
    expect(
      (screen.getByRole('button', { name: 'Lower priority – First' }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
  });

  /**
   * Reordering sends the whole list in its new order, not a single moved id:
   * `PUT /rules/reorder` rewrites every priority as `(index + 1) * 10`, so a
   * partial list would renumber the rules it was not given.
   */
  it('sends the full new order when a rule moves', async () => {
    const reorder = vi.spyOn(api, 'reorderRules').mockResolvedValue(undefined as never);
    show([rule({ id: 'r1', name: 'First' }), rule({ id: 'r2', name: 'Last' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Raise priority – Last' }));

    await waitFor(() => expect(reorder).toHaveBeenCalledTimes(1));
    expect(nthCall(reorder)[0]).toEqual(['r2', 'r1']);
  });

  it('asks before deleting, naming the rule', async () => {
    const remove = vi.spyOn(api, 'deleteRule');
    show([rule({ name: 'Japanese animation' })]);

    await fireEvent.click(
      await screen.findByRole('button', { name: 'Delete – Japanese animation' }),
    );

    expect(await answerConfirmation(null)).toBe('Delete the rule "Japanese animation"?');
    expect(remove).not.toHaveBeenCalled();
  });

  /**
   * The banner stays until something replaces it. Left standing across the
   * next action, a success reads as that action's outcome, printed above the
   * reason it was refused, and the older, louder sentence is the one believed.
   */
  it('takes the previous success off screen when the next action is refused', async () => {
    vi.spyOn(api, 'duplicateRule').mockResolvedValue(undefined as never);
    vi.spyOn(api, 'deleteRule').mockRejectedValue(
      new ApiError('The rule is pinned by a test case', 409, 'conflict'),
    );
    show([rule({ id: 'r1', name: 'Anime' }), rule({ id: 'r2', name: 'Kids', priority: 20 })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Duplicate – Anime' }));
    expect(await screen.findByText('Rule duplicated')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – Kids' }));
    await answerConfirmation();

    expect(await screen.findByText('The rule is pinned by a test case')).toBeTruthy();
    expect(screen.queryByText('Rule duplicated')).toBeNull();
  });

  /**
   * A cancelled question did nothing, so it has nothing to take off screen. The
   * last outcome is still the true one.
   */
  it('leaves the previous success standing when an import is cancelled', async () => {
    vi.spyOn(api, 'duplicateRule').mockResolvedValue(undefined as never);
    const importRules = vi.spyOn(api, 'importRules');
    const { container } = show([rule({ id: 'r1', name: 'Anime' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Duplicate – Anime' }));
    expect(await screen.findByText('Rule duplicated')).toBeTruthy();

    const input = container.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File(['{"version":1,"rules":[]}'], 'rules.json', { type: 'application/json' });
    await fireEvent.change(input, { target: { files: [file] } });
    await answerConfirmation(null);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(importRules).not.toHaveBeenCalled();
    expect(screen.getByText('Rule duplicated')).toBeTruthy();
  });

  it('deletes once the question is answered', async () => {
    const remove = vi.spyOn(api, 'deleteRule').mockResolvedValue(undefined as never);
    show([rule()]);

    await fireEvent.click(await screen.findByRole('button', { name: /^Delete – / }));
    await answerConfirmation();

    await waitFor(() => expect(remove).toHaveBeenCalledWith('r1'));
  });

  /**
   * A disabled rule is dimmed and marked, not hidden. Hiding it makes a routing
   * that "should" match and does not look like a bug in the engine.
   */
  it('marks a disabled rule rather than hiding it', async () => {
    show([rule({ enabled: false, name: 'Japanese animation' })]);

    expect(await screen.findByText('Japanese animation')).toBeTruthy();
    expect(screen.getByText('disabled')).toBeTruthy();
  });

  it('shows an exclusion as a veto, not as another condition', async () => {
    show([
      rule({
        conditions: [{ type: 'genre_contains', value: ['Animation'] }],
        exclusions: [{ type: 'genre_contains', value: ['Documentary'] }],
      }),
    ]);

    await screen.findByText('Japanese animation');
    // An exclusion vetoes a rule that otherwise matched; reading it as a third
    // condition inverts what the rule does.
    expect(screen.getByText(/except/)).toBeTruthy();
  });

  /**
   * The builder is driven by the catalogue the backend serves, so a condition
   * added in Rust needs no frontend change. Opening the editor must therefore
   * wait for the catalogue rather than render an empty picker.
   */
  it('builds the editor from the catalogue the server serves', async () => {
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New rule' }));

    // Offered in both pickers — conditions and exclusions — which is itself the
    // point: one catalogue drives both lists.
    expect((await screen.findAllByText('Genre contains')).length).toBeGreaterThan(0);
  });

  async function importFile(result: { imported: number; skipped: string[] }) {
    vi.spyOn(api, 'importRules').mockResolvedValue(result);
    const { container } = show([rule({ id: 'r1', name: 'Anime' })]);
    await screen.findByRole('button', { name: 'Duplicate – Anime' });

    const input = container.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File(['{"version":1,"rules":[]}'], 'rules.json', { type: 'application/json' });
    await fireEvent.change(input, { target: { files: [file] } });
    await answerConfirmation('append');
  }

  /** An import that brought in no rule at all is a failure, not a count of zero. */
  it('reports an import that skipped every rule as a failure', async () => {
    await importFile({ imported: 0, skipped: ["'Anime': no condition", "'Kids': no target"] });

    const summary = await screen.findByText('Rules imported: 0, skipped: 2');
    expect(summary.closest('.banner')?.classList.contains('banner-danger')).toBe(true);
    expect(screen.getByText("'Kids': no target")).toBeTruthy();
  });

  /** Some rules in and some refused is a partial result, each refusal with its reason. */
  it('reports an import that skipped some rules as a partial result', async () => {
    await importFile({ imported: 1, skipped: ["'Kids': no target"] });

    const summary = await screen.findByText('Rules imported: 1, skipped: 1');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText("'Kids': no target")).toBeTruthy();
  });
});
