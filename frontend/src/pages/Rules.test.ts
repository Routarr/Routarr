import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api, ApiError } from '../api/client';
import type { Category, ConditionCatalog, LibraryFacets, Rule, RuleHealth } from '../api/types';
import Rules from './Rules.svelte';
import { answerConfirmation } from '../test/confirm';
import { statusRevision } from '../lib/status.svelte';
import { instance, rule, template, templatePreview } from '../test/fixtures';

const catalog: ConditionCatalog = {
  match_modes: ['all', 'any'],
  conditions: [
    {
      type: 'genre_contains',
      label: 'Genre contains',
      group: 'metadata',

      label_key: 'ConditionLabelGenreContains',
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
  score_scales: [{ id: 'imdb', name: 'IMDb', max: 10, votes: true }],
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

/** Two Radarr, so a rule kept to one of them names which. */
const INSTANCES = [instance(), instance({ id: 'i2', name: 'Radarr 4K' })];

const NO_FACETS = {
  total_media: 0,
  without_metadata: 0,
  vocabularies: { original_languages: [], origin_countries: [] },
  genres: [],
  original_languages: [],
  origin_countries: [],
  certifications: [],
  tags: [],
  series_types: [],
  quality_profiles: [],
  studios: [],
  networks: [],
  collections: [],
  root_folders: [],
  statuses: [],
} as unknown as LibraryFacets;

/** The diagnostics and the templates answer unless a test has already said otherwise. */
function show(rules: Rule[], served: ConditionCatalog = catalog, language = 'en') {
  vi.spyOn(api, 'getRules').mockResolvedValue(rules);
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  vi.spyOn(api, 'getConditionCatalog').mockResolvedValue(served);
  vi.spyOn(api, 'getInstances').mockResolvedValue(INSTANCES);
  if (!vi.isMockFunction(api.getRuleHealth)) {
    vi.spyOn(api, 'getRuleHealth').mockResolvedValue({ total_media: 0, rules: [] });
  }
  if (!vi.isMockFunction(api.getLibraryFacets)) {
    vi.spyOn(api, 'getLibraryFacets').mockResolvedValue(NO_FACETS);
  }
  if (!vi.isMockFunction(api.getTemplates)) {
    vi.spyOn(api, 'getTemplates').mockResolvedValue([]);
  }
  return renderWithI18n(Rules, { language });
}

afterEach(() => vi.restoreAllMocks());

/** Open a rule's action menu and pick one of its items. */
async function rowAction(rule: string | RegExp, action: string) {
  const name = typeof rule === 'string' ? `Actions – ${rule}` : rule;
  await fireEvent.click(await screen.findByRole('button', { name }));
  await fireEvent.click(await screen.findByRole('menuitem', { name: action }));
}

describe('Rules', () => {
  /** A rule kept to some instances never fires on the others, which its row has to say. */
  it('names the instances a rule is kept to beside its media type', async () => {
    show([rule({ instance_ids: ['i2'] })]);

    const row = await screen.findByRole('row', { name: /Japanese animation/ });
    expect(within(row).getByText('Radarr 4K')).toBeTruthy();
    expect(within(row).queryByText('Radarr')).toBeNull();
  });

  /** The two diagnostics failing silently would look exactly like a healthy rule set. */
  it('says the rule analysis is missing, and reads it again on Retry', async () => {
    const report = vi
      .spyOn(api, 'getRuleHealth')
      .mockRejectedValueOnce(new Error('timed out'))
      .mockResolvedValue({ total_media: 0, rules: [] });
    show([rule({ id: 'r1', name: 'Anime' })]);

    const warning = await screen.findByText(
      'The rule analysis could not be read, so no badge says which rules decide nothing and the library panel is empty.',
    );
    await fireEvent.click(within(warning.closest('.banner') as HTMLElement).getByText('Retry'));

    await waitFor(() =>
      expect(
        screen.queryByText(
          'The rule analysis could not be read, so no badge says which rules decide nothing and the library panel is empty.',
        ),
      ).toBeNull(),
    );
    expect(report).toHaveBeenCalledTimes(2);
  });

  it('says nothing of the kind when the analysis answers', async () => {
    show([rule({ id: 'r1', name: 'Anime' })]);
    await screen.findByRole('button', { name: 'Actions – Anime' });
    await waitFor(() => expect(api.getLibraryFacets).toHaveBeenCalled());

    expect(
      screen.queryByText(
        'The rule analysis could not be read, so no badge says which rules decide nothing and the library panel is empty.',
      ),
    ).toBeNull();
  });

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

    expect(
      await screen.findByText('No rules yet. Every media item falls back to the default category.'),
    ).toBeTruthy();
  });

  it('names each row action after its rule', async () => {
    show([rule({ name: 'Japanese animation' }), rule({ id: 'r2', name: 'Kids' })]);

    await screen.findByText('Japanese animation');
    for (const name of ['Edit – Kids', 'Actions – Kids']) {
      expect(screen.getByRole('button', { name })).toBeTruthy();
    }
  });

  /** Raising the first or lowering the last would send a reorder that changes nothing. */
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
   * The reorder rewrites every priority from the list's order, so a partial list would
   * renumber the rules it was not given.
   */
  it('sends the full new order when a rule moves', async () => {
    const reorder = vi.spyOn(api, 'reorderRules').mockResolvedValue(undefined as never);
    show([rule({ id: 'r1', name: 'First' }), rule({ id: 'r2', name: 'Last' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Raise priority – Last' }));

    await waitFor(() => expect(reorder).toHaveBeenCalledTimes(1));
    expect(nthCall(reorder)[0]).toEqual(['r2', 'r1']);
  });

  /**
   * An action keeps the table and the focus with the rule, or a keyboard user starts again from
   * the top. The library facets are read once, since no action on a rule changes them.
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

    await rowAction('Japanese animation', 'Delete');

    expect(await answerConfirmation(null)).toBe('Delete the rule "Japanese animation"?');
    expect(remove).not.toHaveBeenCalled();
  });

  /** A stale success above the next action's refusal reads as that action's outcome. */
  it('takes the previous success off screen when the next action is refused', async () => {
    vi.spyOn(api, 'duplicateRule').mockResolvedValue(undefined as never);
    vi.spyOn(api, 'deleteRule').mockRejectedValue(
      new ApiError('The rule is pinned by a test case', 409, 'conflict'),
    );
    show([rule({ id: 'r1', name: 'Anime' }), rule({ id: 'r2', name: 'Kids', priority: 20 })]);

    await rowAction('Anime', 'Duplicate (disabled copy)');
    expect(await screen.findByText('Rule duplicated')).toBeTruthy();

    await rowAction('Kids', 'Delete');
    await answerConfirmation();

    expect(await screen.findByText('The rule is pinned by a test case')).toBeTruthy();
    expect(screen.queryByText('Rule duplicated')).toBeNull();
  });

  /** A cancelled question did nothing, so the last outcome stays on screen. */
  it('leaves the previous success standing when an import is cancelled', async () => {
    vi.spyOn(api, 'duplicateRule').mockResolvedValue(undefined as never);
    const importRules = vi.spyOn(api, 'importRules');
    const { container } = show([rule({ id: 'r1', name: 'Anime' })]);

    await rowAction('Anime', 'Duplicate (disabled copy)');
    expect(await screen.findByText('Rule duplicated')).toBeTruthy();

    const input = container.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File(['{"version":1,"rules":[]}'], 'rules.json', { type: 'application/json' });
    await fireEvent.change(input, { target: { files: [file] } });
    await answerConfirmation(null);

    expect(importRules).not.toHaveBeenCalled();
    expect(screen.getByText('Rule duplicated')).toBeTruthy();
  });

  it('deletes once the question is answered', async () => {
    const remove = vi.spyOn(api, 'deleteRule').mockResolvedValue(undefined as never);
    show([rule()]);

    await rowAction(/^Actions – /, 'Delete');
    await answerConfirmation();

    await waitFor(() => expect(remove).toHaveBeenCalledWith('r1'));
  });

  /** The shell draws the guide, whose rule step hears of a rule only through this. */
  it('tells the shell when the rules change', async () => {
    vi.spyOn(api, 'deleteRule').mockResolvedValue(undefined as never);
    show([rule()]);
    const before = statusRevision();

    await rowAction(/^Actions – /, 'Delete');
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
   * A stored rule can still hold a repeated value, and chips keyed by value would crash on it,
   * so the editor drops the repeat.
   */
  it('opens a rule stored with a repeated value, and saves it without the repeat', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const update = vi.spyOn(api, 'updateRule').mockResolvedValue(rule());
    show([
      rule({
        conditions: [{ type: 'genre_contains', value: ['Animation', 'Animation', 'animation'] }],
      }),
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: /^Edit – / }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Save rule' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(nthCall(update)[1].conditions).toEqual([
      { type: 'genre_contains', value: ['Animation'] },
    ]);
  });

  /** Hidden, a disabled rule would make a routing that does not match look like an engine bug. */
  it('marks a disabled rule rather than hiding it', async () => {
    show([rule({ enabled: false, name: 'Japanese animation' })]);

    expect(await screen.findByText('Japanese animation')).toBeTruthy();
    expect(screen.getByText('Disabled')).toBeTruthy();
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
      quality_profiles: [],
      studios: [],
      networks: [],
      collections: [],
      root_folders: [],
      statuses: [],
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

    expect(await screen.findByText('Original language is one of Japanese (ja)')).toBeTruthy();
    expect(screen.getByText('Genres include any of Animation, Family')).toBeTruthy();
    const veto = screen.getByText('Certification is one of 12 (12 and over)').closest('li');
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
      quality_profiles: [],
      studios: [],
      networks: [],
      collections: [],
      root_folders: [],
      statuses: [],
    } as unknown as LibraryFacets);
    show([rule({ conditions: [{ type: 'original_language', value: ['ja'] }] })], served, 'fr');

    expect(await screen.findByText('Original language is one of japonais (ja)')).toBeTruthy();
  });

  /**
   * A held value comes without a name, and `Intl` may have none for its code, so the table
   * takes the vocabulary's name, as the editor does.
   */
  it('names a held value from the vocabulary when the language has no name for it', async () => {
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
      ],
    };
    vi.spyOn(api, 'getLibraryFacets').mockResolvedValue({
      total_media: 12,
      without_metadata: 0,
      vocabularies: {
        original_languages: [{ value: 'qaa', label: 'Reserved (qaa)', count: 0 }],
        origin_countries: [],
      },
      genres: [],
      original_languages: [{ value: 'qaa', count: 3 }],
      origin_countries: [],
      certifications: [],
      tags: [],
      series_types: [],
      quality_profiles: [],
      studios: [],
      networks: [],
      collections: [],
      root_folders: [],
      statuses: [],
    } as unknown as LibraryFacets);
    show([rule({ conditions: [{ type: 'original_language', value: ['qaa'] }] })], served);

    expect(await screen.findByText('Original language is one of Reserved (qaa)')).toBeTruthy();
  });

  /** The editor waits for the catalogue it is built from, rather than draw an empty picker. */
  it('builds the editor from the catalogue the server serves', async () => {
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New rule' }));

    // Found in both pickers, conditions and exclusions, since one catalogue drives both.
    expect((await screen.findAllByText('Genre contains')).length).toBeGreaterThan(0);
  });

  async function importFile(
    result: { imported: number; skipped: string[]; adjusted?: string[] },
    answer: 'append' | 'replace' = 'append',
  ) {
    const importRules = vi.spyOn(api, 'importRules').mockResolvedValue({ adjusted: [], ...result });
    const { container } = show([rule({ id: 'r1', name: 'Anime' })]);
    await screen.findByRole('button', { name: 'Actions – Anime' });

    const input = container.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File(['{"version":1,"rules":[]}'], 'rules.json', { type: 'application/json' });
    await fireEvent.change(input, { target: { files: [file] } });
    await answerConfirmation(answer);
    return importRules;
  }

  /** Replace deletes every rule first, so the answer must reach the request as given. */
  it('adds the file to the rules when Append is chosen', async () => {
    const importRules = await importFile({ imported: 1, skipped: [] });
    await waitFor(() => expect(importRules).toHaveBeenCalledTimes(1));
    expect(nthCall(importRules)[1]).toBe(false);
  });

  it('replaces the rules only when Replace all is chosen', async () => {
    const importRules = await importFile({ imported: 1, skipped: [] }, 'replace');
    await waitFor(() => expect(importRules).toHaveBeenCalledTimes(1));
    expect(nthCall(importRules)[1]).toBe(true);
  });

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

  /** A rule imported switched off or on fewer instances than its file names needs a look. */
  it('names each rule imported otherwise than its file has it', async () => {
    const note = "'Anime': imported switched off";
    await importFile({ imported: 1, skipped: [], adjusted: [note] });

    const summary = await screen.findByText('Rules imported: 1');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText(note)).toBeTruthy();
  });

  /** The guide's link waits for the catalogue the editor is built from, as the button does. */
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

describe('the templates', () => {
  /** Above the table while it is empty, below it once a rule is written. */
  it('offers the templates first while no rule is written, and after the rules once one is', async () => {
    vi.spyOn(api, 'getTemplates').mockResolvedValue([template()]);
    const { unmount } = show([]);
    const gallery = await screen.findByRole('heading', { name: 'Templates' });
    const table = screen.getByRole('table', { name: 'Rules' });
    expect(gallery.compareDocumentPosition(table) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    unmount();

    show([rule()]);
    const below = await screen.findByRole('heading', { name: 'Templates' });
    const after = screen.getByRole('table', { name: 'Rules' });
    expect(below.compareDocumentPosition(after) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();
  });

  it('says a template is added, names a category made without a folder, and reloads', async () => {
    vi.spyOn(api, 'getTemplates').mockResolvedValue([template()]);
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    vi.spyOn(api, 'importTemplate').mockResolvedValue({
      rules: [rule({ template: 'documentaries@1' })],
      created: ['documentaries'],
      replaced: 0,
    });
    show([]);
    const loads = vi.mocked(api.getRules).mock.calls.length;

    await fireEvent.click(await screen.findByRole('button', { name: 'Add – Documentaries' }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Add' }));

    const summary = await screen.findByText('Template "Documentaries" added. Rules created: 1');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText(/The category "documentaries" has no folder yet/)).toBeTruthy();
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(vi.mocked(api.getRules).mock.calls.length).toBeGreaterThan(loads);
  });

  it('hands the gallery the focus when the guide sends the reader to it', async () => {
    window.history.replaceState({}, '', '/rules#templates');
    vi.spyOn(api, 'getTemplates').mockResolvedValue([template()]);
    show([]);

    const title = await screen.findByRole('heading', { name: 'Templates' });
    await waitFor(() => expect(document.activeElement).toBe(title));
    window.history.replaceState({}, '', '/rules');
  });

  /** A household language the library holds no film in is still named, not a bare code. */
  it('names the languages the household watches in from the whole vocabulary', async () => {
    vi.spyOn(api, 'getLibraryFacets').mockResolvedValue({
      ...NO_FACETS,
      vocabularies: {
        original_languages: [{ value: 'fr', label: 'French (fr)', count: 0 }],
        origin_countries: [],
      },
    });
    vi.spyOn(api, 'getTemplates').mockResolvedValue([
      template({
        id: 'foreign-films',
        name: 'Foreign-language films',
        parameter: 'languages',
        languages: ['fr'],
      }),
    ]);
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    show([]);

    await fireEvent.click(
      await screen.findByRole('button', { name: 'Add – Foreign-language films' }),
    );

    const chips = within(await screen.findByRole('list', { name: 'Selected values' }));
    expect(chips.getByText(/French/)).toBeTruthy();
  });

  /** Below the table once rules are written, the gallery is a link away. */
  it('follows the link to the gallery from the screen it is on', async () => {
    vi.spyOn(api, 'getTemplates').mockResolvedValue([template()]);
    show([rule()]);

    const link = await screen.findByRole('link', { name: 'Templates' });
    expect(link.getAttribute('href')).toBe('/rules#templates');
    window.history.pushState({}, '', '/rules#templates');
    window.dispatchEvent(new HashChangeEvent('hashchange'));

    const title = await screen.findByRole('heading', { name: 'Templates', level: 2 });
    await waitFor(() => expect(document.activeElement).toBe(title));
    window.history.replaceState({}, '', '/rules');
  });
});

/** A deleted rule takes its row and its menu: the next rule's menu takes the focus. */
it('hands the focus to the rule that took the place of the deleted one', async () => {
  const first = rule({ id: 'r1', name: 'First' });
  const next = rule({ id: 'r2', name: 'Next' });
  show([first, next]);
  vi.spyOn(api, 'deleteRule').mockResolvedValue(undefined as never);

  await rowAction('First', 'Delete');
  vi.spyOn(api, 'getRules').mockResolvedValue([next]);
  await answerConfirmation();

  await waitFor(() =>
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Actions – Next' })),
  );
});

/**
 * A rule under a broader one, a copy of another and one that matches nothing each say so on
 * their row, the first two naming the rule to look at.
 */
describe('the rule health badges', () => {
  const verdict = (over: Partial<RuleHealth>): RuleHealth => ({
    rule_id: 'r1',
    rule_name: 'Anime',
    priority: 10,
    enabled: true,
    won: 0,
    shadowed: 0,
    vetoed: 0,
    shadowed_by: null,
    matched_nothing: false,
    ambiguous_with: null,
    duplicate_of: null,
    ...over,
  });

  it.each([
    [
      'a shadowed rule names the rule taking its items',
      { shadowed: 4, shadowed_by: 'Movies' },
      'shadowed by "Movies"',
    ],
    [
      'a duplicate names the rule it repeats',
      { duplicate_of: 'Anime copy' },
      'same as "Anime copy"',
    ],
    ['a rule that matches nothing says so', { matched_nothing: true }, 'matches nothing'],
  ])('%s', async (_, over, badge) => {
    vi.spyOn(api, 'getRuleHealth').mockResolvedValue({
      total_media: 12,
      rules: [verdict(over)],
    });
    show([rule({ id: 'r1', name: 'Anime' })]);

    const row = (await screen.findByText('Anime')).closest('tr') as HTMLElement;
    expect(await within(row).findByText(badge)).toBeTruthy();
  });

  /** How many it lost and to whom is said in the row, not in a title only a mouse shows. */
  it('says in the row how many items a shadowed rule loses', async () => {
    vi.spyOn(api, 'getRuleHealth').mockResolvedValue({
      total_media: 12,
      rules: [verdict({ shadowed: 4, shadowed_by: 'Movies' })],
    });
    show([rule({ id: 'r1', name: 'Anime' })]);

    const row = (await screen.findByText('Anime')).closest('tr') as HTMLElement;
    expect(
      await within(row).findByText('Items matched and never won: 4. "Movies" takes them first.'),
    ).toBeTruthy();
  });

  it('marks nothing on a rule that decides', async () => {
    vi.spyOn(api, 'getRuleHealth').mockResolvedValue({
      total_media: 12,
      rules: [verdict({ won: 3 })],
    });
    show([rule({ id: 'r1', name: 'Anime' })]);

    const row = (await screen.findByText('Anime')).closest('tr') as HTMLElement;
    await waitFor(() => expect(api.getRuleHealth).toHaveBeenCalled());
    expect(within(row).queryByText(/shadowed by|same as|matches nothing/)).toBeNull();
  });
});
