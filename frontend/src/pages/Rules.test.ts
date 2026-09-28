import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api, ApiError } from '../api/client';
import type { Category, ConditionCatalog, LibraryFacets, Rule } from '../api/types';
import Rules from './Rules.svelte';
import { answerConfirmation } from '../test/confirm';
import { statusRevision } from '../lib/status.svelte';

/**
 * First match by ascending priority wins, so the order of this table *is* the
 * routing. Reordering it rewrites priorities on the server; nothing about it is
 * cosmetic.
 */

const STRINGS = {
  RulesEngine: 'Rules',
  NewRule: 'New rule',
  Dismiss: 'Dismiss',
  NoRulesYet: 'No rule yet',
  RaisePriority: 'Raise priority',
  LowerPriority: 'Lower priority',
  Edit: 'Edit',
  SaveRule: 'Save rule',
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
  ListSeparator: ', ',
  None: 'none',
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

function show(rules: Rule[], served: ConditionCatalog = catalog, language = 'en') {
  vi.spyOn(api, 'getRules').mockResolvedValue(rules);
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  vi.spyOn(api, 'getConditionCatalog').mockResolvedValue(served);
  return renderWithI18n(Rules, { strings: STRINGS, language });
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  window.history.replaceState({}, '', '/');
});

describe('Rules', () => {
  /** A label over a hidden input takes no focus, so the import is a button. */
  it('offers the import as a button that opens the file picker', async () => {
    show([]);
    const pick = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(() => {});
    const button = await screen.findByRole('button', { name: 'Import' });

    button.focus();
    expect(document.activeElement).toBe(button);
    await fireEvent.click(button);
    expect(pick).toHaveBeenCalledTimes(1);
  });

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

  /**
   * An action on a rule keeps the table on screen and the focus with the rule.
   * Swapped for a skeleton at each reload, the rows took the focus to the page
   * and a keyboard user started again from the top. The library facets are
   * the screen's, read once: no action on a rule changes them.
   */
  it("moving a rule keeps the table and the focus on the moved rule's button", async () => {
    const reorder = vi.spyOn(api, 'reorderRules').mockResolvedValue(undefined as never);
    const facets = vi.spyOn(api, 'getLibraryFacets').mockRejectedValue(new Error('not read'));
    show([rule({ id: 'r1', name: 'First' }), rule({ id: 'r2', name: 'Last' })]);
    const raise = await screen.findByRole('button', { name: 'Raise priority – Last' });
    let answer: (rules: Rule[]) => void = () => {};
    vi.spyOn(api, 'getRules').mockReturnValue(new Promise((resolve) => (answer = resolve)));

    raise.focus();
    await fireEvent.click(raise);
    await waitFor(() => expect(reorder).toHaveBeenCalledTimes(1));

    // While the list reloads, its rows stay.
    expect(screen.getByRole('button', { name: 'Lower priority – Last' })).toBeTruthy();
    answer([rule({ id: 'r2', name: 'Last' }), rule({ id: 'r1', name: 'First' })]);
    // At the top it cannot rise further, so its other arrow takes the focus.
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Lower priority – Last' }),
      ),
    );
    expect(facets).toHaveBeenCalledTimes(1);
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

  /** The shell draws the guide, whose rule step hears of a rule only through this. */
  it('tells the shell when the rules change', async () => {
    vi.spyOn(api, 'deleteRule').mockResolvedValue(undefined as never);
    show([rule()]);
    const before = statusRevision();

    await fireEvent.click(await screen.findByRole('button', { name: /^Delete – / }));
    await answerConfirmation();

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /** The guide's rule step hears of a saved rule only through the shell. */
  it('tells the shell when a rule is saved from the editor', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const update = vi.spyOn(api, 'updateRule').mockResolvedValue(rule());
    show([rule()]);
    const before = statusRevision();

    await fireEvent.click(await screen.findByRole('button', { name: /^Edit – / }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Save rule' }));

    await waitFor(() => expect(update).toHaveBeenCalledWith('r1', expect.anything()));
    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
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

  /** The table reads as the editor does, never in the engine's own identifiers. */
  it('describes each condition by its caption and each value by its name', async () => {
    const spec = catalog.conditions[0] as ConditionCatalog['conditions'][number];
    const served: ConditionCatalog = {
      ...catalog,
      conditions: [
        ...catalog.conditions,
        {
          ...spec,
          type: 'original_language',
          label: 'Original language is',
          suggestions: 'original_languages',
        },
        {
          ...spec,
          type: 'certification_in',
          label: 'Certification is',
          suggestions: 'certifications',
        },
      ],
    };
    vi.spyOn(api, 'getLibraryFacets').mockResolvedValue({
      total_media: 12,
      without_metadata: 0,
      vocabularies: {
        original_languages: [{ value: 'ja', label: 'Japanese (ja)', count: 0 }],
        origin_countries: [],
      },
      genres: [],
      original_languages: [],
      origin_countries: [],
      certifications: [{ value: '12', label: '12 (12 and over)', count: 11 }],
      tags: [],
      series_types: [],
      root_folders: [],
    } as unknown as LibraryFacets);
    show(
      [
        rule({
          conditions: [
            { type: 'genre_contains', value: ['Animation', 'Family'] },
            { type: 'original_language', value: ['ja'] },
          ],
          exclusions: [{ type: 'certification_in', value: ['12'] }],
        }),
      ],
      served,
    );

    expect(await screen.findByText('Original language is: Japanese (ja)')).toBeTruthy();
    expect(screen.getByText('Genre contains: Animation, Family')).toBeTruthy();
    const veto = screen.getByText('Certification is: 12 (12 and over)').closest('li');
    expect(veto?.classList.contains('is-excluded')).toBe(true);
    expect(veto).toHaveTextContent('except');
    expect(screen.queryByText(/original_language|certification_in/)).toBeNull();
  });

  /** A language is named in the reader's language, not in the server's English. */
  it('names a language value in the language of the interface', async () => {
    const spec = catalog.conditions[0] as ConditionCatalog['conditions'][number];
    const served: ConditionCatalog = {
      ...catalog,
      conditions: [
        ...catalog.conditions,
        {
          ...spec,
          type: 'original_language',
          label: 'Langue originale',
          suggestions: 'original_languages',
        },
      ],
    };
    vi.spyOn(api, 'getLibraryFacets').mockResolvedValue({
      total_media: 12,
      without_metadata: 0,
      vocabularies: {
        original_languages: [{ value: 'ja', label: 'Japanese (ja)', count: 0 }],
        origin_countries: [],
      },
      genres: [],
      original_languages: [],
      origin_countries: [],
      certifications: [],
      tags: [],
      series_types: [],
      root_folders: [],
    } as unknown as LibraryFacets);
    show([rule({ conditions: [{ type: 'original_language', value: ['ja'] }] })], served, 'fr');

    expect(await screen.findByText('Langue originale: japonais (ja)')).toBeTruthy();
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

  /** The editor is built from the catalogue, so the guide's link waits for it as the button does. */
  it('opens the rule editor when the guide sends the reader here', async () => {
    window.history.replaceState({}, '', '/rules?new=1');
    show([]);

    expect(await screen.findByRole('dialog')).toBeTruthy();
    expect(window.location.search).toBe('');
  });

  it('hands the focus to New rule when the editor the guide opened closes', async () => {
    window.history.replaceState({}, '', '/rules?new=1');
    show([]);
    const dialog = await screen.findByRole('dialog');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Dismiss' }));

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'New rule' }));
  });
});
