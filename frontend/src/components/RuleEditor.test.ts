import { describe, it, expect, vi, afterEach } from 'vitest';
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { dropFocus } from '../test/focus';
import { nthCall } from '../test/spy';
import { instance } from '../test/fixtures';
import { unloading } from '../test/leaving';
import { navigate, router } from '../lib/router.svelte';
import { ApiError, api } from '../api/client';
import type { LibraryFacets, PreviewChange, RuleDraft, SimulationSummary } from '../api/types';
import { publishAccess } from '../lib/access.svelte';
import RuleEditor from './RuleEditor.svelte';

/**
 * The editor asks the server about the draft as it is typed, so a condition that can never match
 * is named before Save is pressed rather than found as a rule that silently routes nothing.
 */

const DRAFT: RuleDraft = {
  name: 'Anime',
  priority: 100,
  enabled: true,
  media_type: 'movie',
  conditions: [{ type: 'genre_contains', value: [] }],
  exclusions: [],
  match_mode: 'all',
  target_category: 'anime',
};

const EMPTY_FACETS: LibraryFacets = {
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
};

/** The summary at the foot of the form, which lists every issue, each under its field too. */
const summary = () => within(screen.getByRole('list', { name: 'Validation results' }));

/** Any edit at all: what turns an untouched form into one being built. */
async function touch() {
  await userEvent.type(await screen.findByLabelText('Rule name'), '!');
}

function render(ruleId?: string, extra: Record<string, unknown> = {}) {
  vi.spyOn(api, 'getLibraryFacets').mockResolvedValue(EMPTY_FACETS);
  renderWithI18n(RuleEditor, {
    props: {
      draft: DRAFT,
      ruleId,
      categories: [],
      instances: [
        instance({ id: 'i1', name: 'Radarr 4K' }),
        instance({ id: 'i2', name: 'Sonarr', instance_type: 'sonarr' }),
      ],
      catalog: { conditions: [] },
      onClose: () => {},
      onSaved: async () => {},
      ...extra,
    },
  });
}

afterEach(() => vi.restoreAllMocks());

describe('RuleEditor', () => {
  /** Opened on its close button, a form is one reflex Enter from thrown away. */
  it('opens on the rule name', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render();

    expect(document.activeElement).toBe(await screen.findByLabelText('Rule name'));
  });

  it('names a dead condition before the rule is saved, and holds Save until it is fixed', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [
        {
          severity: 'error',
          field: 'conditions',
          key: 'ValidationYearRangeInverted',
          params: {},
          message: 'Condition 1 (Year): the range is inverted',
        },
      ],
    });
    render();
    await touch();

    await waitFor(() =>
      expect(summary().getByText('Condition 1 (Year): the range is inverted')).toBeInTheDocument(),
    );
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeDisabled();
  });

  /** Chinese writes a note in full-width brackets, with no space before them. */
  it('marks the description optional as the language sets a note', async () => {
    vi.spyOn(api, 'getLibraryFacets').mockResolvedValue(EMPTY_FACETS);
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    renderWithI18n(RuleEditor, {
      props: {
        draft: DRAFT,
        categories: [],
        instances: [],
        catalog: { conditions: [] },
        onClose: () => {},
        onSaved: async () => {},
      },
      language: 'zh_CN',
      strings: { Description: '描述', Optional: '可选', WithNote: '{text}（{note}）' },
    });

    expect(await screen.findByLabelText('描述（可选）')).toBeInTheDocument();
  });

  /** Which way the order runs is said under the field, where a keyboard reaches it too. */
  it('says under the priority which rule is evaluated first', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render();

    expect(await screen.findByLabelText('Priority')).toHaveAccessibleDescription(
      'Lower numbers are evaluated first',
    );
    // As text on the page: a title describes the field too, to a mouse alone.
    expect(screen.getByText('Lower numbers are evaluated first')).toBeInTheDocument();
  });

  it('holds Save on a fractional priority and says why', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render();
    const priority = await screen.findByLabelText('Priority');

    await userEvent.clear(priority);
    await userEvent.type(priority, '1.5');

    expect(screen.getByText('Enter a whole number.')).toBeInTheDocument();
    expect(priority).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeDisabled();

    await userEvent.clear(priority);
    await userEvent.type(priority, '15');
    expect(screen.queryByText('Enter a whole number.')).toBeNull();
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeEnabled();
  });

  it('shows a warning without holding Save', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: true,
      issues: [
        {
          severity: 'warning',
          field: 'target_category',
          key: 'ValidationCategoryUnmapped',
          params: {},
          message: 'anime is not mapped',
        },
      ],
    });
    render();
    await touch();

    await waitFor(() => expect(screen.getByText('anime is not mapped')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeEnabled();
  });

  /**
   * The server names the code a language written as its name stands for, and the reader chooses:
   * the value is saved as written, never rewritten behind them.
   */
  it('names the code of a language written as a name, and saves the value as written', async () => {
    const message =
      "Condition 1 (Original language): 'Japanese' is not the code the sources write. Use 'ja' instead.";
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: true,
      issues: [
        {
          severity: 'warning',
          field: 'conditions',
          key: 'ValidationValueNotCode',
          params: { value: 'Japanese', code: 'ja' },
          message,
        },
      ],
    });
    const update = vi.spyOn(api, 'updateRule').mockResolvedValue(undefined as never);
    const language = { type: 'original_language', value: ['Japanese'] };
    render('r1', {
      draft: { ...DRAFT, conditions: [language] },
      catalog: {
        conditions: [
          {
            type: 'original_language',
            label: 'Original language',
            value_type: 'string_list',
            needs_metadata: true,
            suggestions: '',
            quantifier: '',
            counterpart: '',
            media_types: ['movie', 'series'],
            metadata_field: 'original_language',
            available: true,
          },
        ],
      },
    });

    const warning = await screen.findByText(message);
    expect(warning.closest('li')).toHaveClass('banner-warning');
    const save = screen.getByRole('button', { name: 'Save rule' });
    expect(save).toBeEnabled();
    await userEvent.click(save);

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(nthCall(update)[1]).toMatchObject({ conditions: [language] });
  });

  it('asks the server once the draft settles, not on every keystroke', async () => {
    const validate = vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render();
    await waitFor(() => expect(validate).toHaveBeenCalledTimes(1));

    // Four keystrokes in a row: without the debounce each one asks, so the count pins it.
    await userEvent.type(await screen.findByLabelText('Rule name'), 'four');

    await waitFor(() => expect(validate).toHaveBeenCalledTimes(2));
    expect(validate.mock.lastCall?.[0]).toMatchObject({ name: 'Anime' + 'four' });
  });

  it('keeps speaking once an edit has been undone', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [
        {
          severity: 'error',
          field: 'name',
          key: 'ValidationNameEmpty',
          params: {},
          message: 'A rule needs a name',
        },
      ],
    });
    render();

    const field = await screen.findByLabelText('Rule name');
    await userEvent.type(field, '!');
    await waitFor(() => expect(summary().getByText('A rule needs a name')).toBeInTheDocument());

    await userEvent.clear(field);
    await userEvent.type(field, 'Anime');

    expect(summary().getByText('A rule needs a name')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeDisabled();
  });

  it('says nothing about a new rule until it is touched', async () => {
    const validate = vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [
        {
          severity: 'error',
          field: 'name',
          key: 'ValidationNameEmpty',
          params: {},
          message: 'A rule needs a name',
        },
      ],
    });
    render();

    // The question is still asked, and the answer is simply not thrown at anyone.
    await waitFor(() => expect(validate).toHaveBeenCalled());
    expect(screen.queryByText('A rule needs a name')).toBeNull();
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeEnabled();

    await touch();
    await waitFor(() => expect(summary().getByText('A rule needs a name')).toBeInTheDocument());
  });

  it('speaks immediately about a rule that already exists', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [
        {
          severity: 'error',
          field: 'target_category',
          key: 'CategoryNotFound',
          params: {},
          message: 'anime no longer exists',
        },
      ],
    });
    render('rule-1');

    await waitFor(() => expect(summary().getByText('anime no longer exists')).toBeInTheDocument());
  });

  it('keeps the name marked required', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    // Through the helper, which stubs `getLibraryFacets`: rendering directly sends a real request.
    render();

    expect(document.querySelector('#rules-rule-name')?.hasAttribute('required')).toBe(true);
  });

  it('keeps quiet about a condition with no value until Save is pressed', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [
        {
          severity: 'error',
          field: 'conditions',
          key: 'ValidationConditionEmpty',
          params: {},
          message: 'Condition 1 (Genre contains) has no value',
        },
      ],
    });
    const create = vi.spyOn(api, 'createRule').mockResolvedValue(undefined as never);
    render();
    await touch();
    await waitFor(() => expect(api.validateRule).toHaveBeenCalled());

    const save = screen.getByRole('button', { name: 'Save rule' });
    expect(screen.queryByText('Condition 1 (Genre contains) has no value')).toBeNull();
    expect(save).toBeEnabled();

    await fireEvent.click(save);

    await waitFor(() =>
      expect(summary().getByText('Condition 1 (Genre contains) has no value')).toBeInTheDocument(),
    );
    expect(create).not.toHaveBeenCalled();
    expect(save).toBeDisabled();
  });

  /** A value picked within the last debounce: Save reads the fresh verdict. */
  it('asks again on Save, and saves a condition that has its value by then', async () => {
    vi.spyOn(api, 'validateRule')
      .mockResolvedValueOnce({
        valid: false,
        issues: [
          {
            severity: 'error',
            field: 'conditions',
            key: 'ValidationConditionEmpty',
            params: {},
            message: 'Condition 1 (Genre contains) has no value',
          },
        ],
      })
      .mockResolvedValue({ valid: true, issues: [] });
    const create = vi.spyOn(api, 'createRule').mockResolvedValue(undefined as never);
    render();
    await touch();
    await waitFor(() => expect(api.validateRule).toHaveBeenCalledTimes(1));

    await fireEvent.click(screen.getByRole('button', { name: 'Save rule' }));

    await waitFor(() => expect(create).toHaveBeenCalled());
  });

  /** Every field the form edits travels in the request, or a rule saves as it was. */
  it('saves the media type, priority, match mode, enabled flag and exclusions as edited', async () => {
    const user = userEvent.setup();
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const update = vi.spyOn(api, 'updateRule').mockResolvedValue(undefined as never);
    const exclusion = { type: 'genre_contains', value: ['Music'] };
    render('r1', { draft: { ...DRAFT, exclusions: [exclusion] } });

    await user.selectOptions(await screen.findByLabelText('Applies to'), 'series');
    const priority = screen.getByLabelText('Priority');
    await user.clear(priority);
    await user.type(priority, '42');
    await user.selectOptions(screen.getByLabelText('Condition logic'), 'any');
    await user.click(screen.getByLabelText('Enabled'));
    await user.click(screen.getByRole('button', { name: 'Save rule' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const [id, saved] = nthCall(update);
    expect(id).toBe('r1');
    expect(saved).toMatchObject({
      media_type: 'series',
      priority: 42,
      match_mode: 'any',
      enabled: false,
      exclusions: [exclusion],
    });
  });

  it('answers the first Save press instead of sending a draft it knows is refused', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [
        {
          severity: 'error',
          field: 'name',
          key: 'ValidationNameEmpty',
          params: {},
          message: 'A rule needs a name',
        },
      ],
    });
    const create = vi.spyOn(api, 'createRule').mockResolvedValue(undefined as never);
    render();
    await waitFor(() => expect(api.validateRule).toHaveBeenCalled());

    const save = screen.getByRole('button', { name: 'Save rule' });
    save.focus();
    await fireEvent.click(save);

    // Under its field, which takes the focus: the dialog scrolls, the summary is a screen away.
    const name = screen.getByLabelText('Rule name');
    await waitFor(() => expect(name).toHaveFocus());
    expect(name).toHaveAttribute('aria-invalid', 'true');
    expect(name).toHaveAccessibleDescription('A rule needs a name');
    expect(create).not.toHaveBeenCalled();

    const summary = screen.getByRole('list', { name: 'Validation results' });
    screen.getByLabelText('Priority').focus();
    await userEvent.click(within(summary).getByRole('button', { name: 'A rule needs a name' }));
    expect(name).toHaveFocus();
  });

  it('says the issue of a condition under its row and moves there on a refused Save', async () => {
    const message = 'Condition 1 (Original language) has no value';
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [
        {
          severity: 'error',
          field: 'conditions',
          key: 'ValidationConditionEmpty',
          params: { section: 'conditions', index: '1', kind: 'original_language' },
          message,
        },
      ],
    });
    render(undefined, {
      draft: { ...DRAFT, conditions: [{ type: 'original_language', value: [] }] },
      catalog: {
        conditions: [
          {
            type: 'original_language',
            label: 'Original language',
            value_type: 'string_list',
            needs_metadata: true,
            suggestions: '',
            quantifier: '',
            counterpart: '',
            media_types: ['movie', 'series'],
            metadata_field: 'original_language',
            available: true,
          },
        ],
      },
    });
    await touch();
    await waitFor(() => expect(api.validateRule).toHaveBeenCalled());

    const save = screen.getByRole('button', { name: 'Save rule' });
    save.focus();
    await fireEvent.click(save);

    const value = screen.getByLabelText('Original language');
    await waitFor(() => expect(value).toHaveFocus());
    expect(value).toHaveAttribute('aria-invalid', 'true');
    expect(value).toHaveAccessibleDescription(new RegExp(message.replace(/[()]/g, '\\$&')));
  });

  /** Save turns disabled while it writes, which drops its focus out of the dialog. */
  it('gives the focus back to Save once a save is refused', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    let refuse = () => {};
    vi.spyOn(api, 'createRule').mockReturnValue(
      new Promise((_, reject) => (refuse = () => reject(new ApiError('Refused', 400, 'invalid')))),
    );
    render();
    await touch();

    const save = screen.getByRole('button', { name: 'Save rule' });
    await userEvent.click(save);
    await waitFor(() => expect(save).toBeDisabled());
    dropFocus();
    refuse();

    await waitFor(() => expect(document.activeElement).toBe(save));
  });
});

describe('the instances a rule is kept to', () => {
  it('sends the instances ticked, and none once every box is cleared', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const create = vi.spyOn(api, 'createRule').mockResolvedValue(undefined as never);
    render();

    const radarr = await screen.findByRole('checkbox', { name: 'Radarr 4K' });
    await userEvent.click(radarr);
    await userEvent.click(screen.getByRole('button', { name: 'Save rule' }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(nthCall(create)[0]).toMatchObject({ instance_ids: ['i1'] });

    await userEvent.click(radarr);
    await userEvent.click(screen.getByRole('button', { name: 'Save rule' }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
    expect(nthCall(create, 1)[0]).toMatchObject({ instance_ids: null });
  });

  it('opens a rule with the instances it is kept to ticked', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render('r1', { draft: { ...DRAFT, instance_ids: ['i2'] } });

    expect(await screen.findByRole('checkbox', { name: 'Sonarr' })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Radarr 4K' })).not.toBeChecked();
  });
});

describe('the facets the parent already holds', () => {
  it('are used as they are, without a second request', async () => {
    render(undefined, {
      knownFacets: { ...EMPTY_FACETS, total_media: 3, genres: [{ value: 'Animation', count: 3 }] },
    });
    await screen.findByLabelText('Rule name');

    expect(api.getLibraryFacets).not.toHaveBeenCalled();
  });

  /** `/rules?new=1` opens the editor once the rules are in, the facets maybe not yet. */
  it('are offered once they arrive after the editor opened, and its own read is dropped', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    let dropped = false;
    vi.spyOn(api, 'getLibraryFacets').mockImplementation(
      (signal) =>
        new Promise((_, reject) =>
          signal?.addEventListener('abort', () => {
            dropped = true;
            reject(new DOMException('aborted', 'AbortError'));
          }),
        ),
    );
    const genres = {
      type: 'genre_contains',
      label: 'Genre contains',
      value_type: 'string_list',
      needs_metadata: false,
      suggestions: 'genres',
      quantifier: '',
      counterpart: '',
      media_types: ['movie', 'series'],
      metadata_field: '',
      available: true,
    };
    const props = {
      draft: DRAFT,
      categories: [],
      instances: [],
      catalog: { conditions: [genres] },
      onClose: () => {},
      onSaved: async () => {},
    };
    const { rerender } = renderWithI18n(RuleEditor, { props: { ...props, knownFacets: null } });
    await screen.findByLabelText('Rule name');

    await rerender({
      ...props,
      knownFacets: { ...EMPTY_FACETS, genres: [{ value: 'Animation', count: 3 }] },
    });
    await userEvent.click(screen.getByLabelText('Genre contains'));

    expect(await screen.findByRole('option', { name: /Animation/ })).toBeInTheDocument();
    expect(dropped).toBe(true);
  });
});

describe('the impact preview', () => {
  const summary = (moves: number): SimulationSummary => ({
    total_media: 12,
    moves_required: moves,
    already_correct: 12 - moves,
    no_category_match: 0,
    skipped_unmapped: 0,
    excluded_by_rule: 0,
  });

  const change = (over: Partial<PreviewChange> = {}): PreviewChange => ({
    media_id: 'm1',
    media_title: 'Akira',
    media_type: 'movie',
    instance_name: 'Radarr',
    from_category: 'films',
    to_category: 'anime',
    current_root_folder: '/films',
    target_root_folder: '/anime',
    reasons: [],
    confidence: 0.9,
    ...over,
  });

  function open(ruleId?: string) {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render(ruleId);
  }

  it('shows what the draft would move, and saves nothing', async () => {
    const preview = vi.spyOn(api, 'previewRule').mockResolvedValue({
      issues: [],
      before: summary(2),
      after: summary(3),
      changed: [change()],
      changed_total: 1,
    });
    const writes = [vi.spyOn(api, 'createRule'), vi.spyOn(api, 'updateRule')];
    open('r1');

    await fireEvent.click(await screen.findByRole('button', { name: 'Preview impact' }));

    expect(
      await screen.findByText(
        'Media items changing category: 1. Moves required would go from 2 to 3, unclassified from 0 to 0.',
      ),
    ).toBeTruthy();
    const row = screen.getByRole('row', { name: /Akira/ });
    expect(row).toHaveTextContent(/films\s*anime/);
    expect(nthCall(preview)).toEqual([expect.objectContaining({ name: 'Anime' }), 'r1']);
    expect(screen.queryByText(/^Changes shown:/)).toBeNull();
    for (const write of writes) expect(write).not.toHaveBeenCalled();
  });

  /** The server sends the first changes only, and the panel does not pass them off as all. */
  it('says the list of changes is cut short when the server sent only part of it', async () => {
    vi.spyOn(api, 'previewRule').mockResolvedValue({
      issues: [],
      before: summary(2),
      after: summary(40),
      changed: [change()],
      changed_total: 38,
    });
    open();

    await fireEvent.click(await screen.findByRole('button', { name: 'Preview impact' }));

    expect(await screen.findByText('Changes shown: 1 of 38.')).toBeTruthy();
  });

  it('says a preview the server refused, and keeps the draft', async () => {
    vi.spyOn(api, 'previewRule').mockRejectedValue(
      new ApiError('The simulation is already running', 409, 'conflict'),
    );
    open();

    await fireEvent.click(await screen.findByRole('button', { name: 'Preview impact' }));

    expect(await screen.findByText('The simulation is already running')).toBeTruthy();
    expect(screen.getByLabelText('Rule name')).toHaveValue('Anime');
  });

  /** The preview button turns disabled while it asks, which drops its focus out of the dialog. */
  it('gives the focus back to the preview button once the preview is shown', async () => {
    let answer = () => {};
    vi.spyOn(api, 'previewRule').mockReturnValue(
      new Promise((resolve) => {
        answer = () =>
          resolve({
            issues: [],
            before: summary(2),
            after: summary(3),
            changed: [],
            changed_total: 0,
          });
      }),
    );
    open();

    const button = await screen.findByRole('button', { name: 'Preview impact' });
    await userEvent.click(button);
    await waitFor(() => expect(button).toBeDisabled());
    dropFocus();
    answer();

    await waitFor(() => expect(document.activeElement).toBe(button));
  });
});

describe('closing the editor', () => {
  it('closes at once when nothing was changed', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const onClose = vi.fn();
    render(undefined, { onClose });

    await userEvent.click(await screen.findByRole('button', { name: 'Cancel' }));

    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['Cancel', 'Cancel'],
    ['the close button', 'Dismiss'],
  ])('keeps a changed rule open when %s is answered with Cancel', async (_, button) => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const onClose = vi.fn();
    render(undefined, { onClose });
    await touch();

    await userEvent.click(screen.getByRole('button', { name: button }));
    expect(await answerConfirmation(null)).toBe('Close the rule without saving your changes?');

    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Rule name')).toHaveValue('Anime!');
  });

  /** Back, a link or a reload drops a half-written rule as surely as Cancel. */
  it('asks before the screen changes under a changed rule, and stays on Cancel', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render();
    expect(unloading()).toBe(false);
    await touch();

    expect(unloading()).toBe(true);
    navigate('/rules');
    expect(await answerConfirmation(null)).toBe('Close the rule without saving your changes?');
    expect(router.path).not.toBe('/rules');
    expect(screen.getByLabelText('Rule name')).toHaveValue('Anime!');
  });

  it('closes a changed rule once the reader agrees to drop it', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const onClose = vi.fn();
    render(undefined, { onClose });
    await touch();

    // Escape reaches the dialog as `cancel`, which jsdom does not raise itself.
    const editor = screen.getAllByRole('dialog')[0]!;
    editor.dispatchEvent(new Event('cancel', { cancelable: true, bubbles: true }));
    await answerConfirmation();

    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

/** The draft holds only the ids it could read, so saving would drop the rest unseen. */
describe('an id list with an entry that is not an id', () => {
  it('holds Save until the entry is corrected', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render(undefined, {
      draft: { ...DRAFT, conditions: [{ type: 'tmdb_id_in', value: [] }] },
      catalog: {
        conditions: [
          {
            type: 'tmdb_id_in',
            label: 'TMDB id is',
            value_type: 'number_list',
            needs_metadata: false,
            suggestions: '',
            quantifier: '',
            counterpart: '',
            media_types: ['movie', 'series'],
            metadata_field: '',
            available: true,
          },
        ],
      },
    });
    const field = await screen.findByLabelText('TMDB id is');
    const save = screen.getByRole('button', { name: 'Save rule' });

    await userEvent.type(field, '603, 6O4');
    await waitFor(() => expect(save).toBeDisabled());

    await userEvent.clear(field);
    await userEvent.type(field, '603, 604');
    await waitFor(() => expect(save).toBeEnabled());
  });
});

describe('a condition field holding what it cannot read', () => {
  const condition = (type: string, valueType: string, label: string) => ({
    type,
    label,
    value_type: valueType,
    needs_metadata: false,
    suggestions: '',
    quantifier: '',
    counterpart: '',
    media_types: ['movie', 'series'],
    metadata_field: '',
    available: true,
  });

  function open(type: string, valueType: string, label: string, value: unknown) {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render(undefined, {
      draft: { ...DRAFT, conditions: [{ type, value }] },
      catalog: { conditions: [condition(type, valueType, label)] },
    });
    return screen.getByRole('button', { name: 'Save rule' });
  }

  it('holds Save on an emptied number, which would otherwise save as 0', async () => {
    const save = open('season_count_over', 'number', 'Season count over', 3);
    const field = await screen.findByLabelText('Season count over');

    await userEvent.clear(field);
    await waitFor(() => expect(save).toBeDisabled());
    expect(field).toHaveAccessibleDescription('Enter a number.');

    await userEvent.type(field, '4');
    await waitFor(() => expect(save).toBeEnabled());
  });

  it('holds Save on a year bound that is not a whole year, and not on an empty one', async () => {
    const save = open('year_between', 'year_range', 'Year between', { min: 1990, max: null });
    const from = await screen.findByLabelText('Year between – from');

    await userEvent.clear(from);
    await userEvent.type(from, '1990.5');
    await waitFor(() => expect(save).toBeDisabled());

    await userEvent.clear(from);
    await waitFor(() => expect(save).toBeEnabled());
  });

  it.each([
    ['boolean', 'has_files', 'Has files'],
    ['string', 'current_root_folder', 'Root folder is'],
    ['string_list', 'genre_contains', 'Genre contains'],
  ])('never holds Save for a %s field', async (valueType, type, label) => {
    const value = valueType === 'boolean' ? true : valueType === 'string' ? '' : [];
    const save = open(type, valueType, label, value);
    const field = await screen.findByLabelText(label);

    if (valueType === 'boolean') await userEvent.selectOptions(field, 'false');
    else await userEvent.type(field, 'x,, 6O4');

    await waitFor(() => expect(screen.getByLabelText('Rule name')).toBeInTheDocument());
    expect(save).toBeEnabled();
  });

  it('lets Save go once the unreadable condition is removed, and keeps another one held', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render(undefined, {
      draft: {
        ...DRAFT,
        conditions: [
          { type: 'tmdb_id_in', value: [] },
          { type: 'tvdb_id_in', value: [] },
        ],
      },
      catalog: {
        conditions: [
          condition('tmdb_id_in', 'number_list', 'TMDB id is'),
          condition('tvdb_id_in', 'number_list', 'TheTVDB id is'),
        ],
      },
    });
    const save = screen.getByRole('button', { name: 'Save rule' });
    await userEvent.type(await screen.findByLabelText('TheTVDB id is'), '6O4');
    await waitFor(() => expect(save).toBeDisabled());

    // The row above goes: the unreadable one moves up and keeps its text.
    await userEvent.click(screen.getByRole('button', { name: 'Delete – TMDB id is' }));
    expect(screen.getByLabelText('TheTVDB id is')).toHaveValue('6O4');
    expect(save).toBeDisabled();

    await userEvent.click(screen.getByRole('button', { name: 'Delete – TheTVDB id is' }));
    await waitFor(() => expect(save).toBeEnabled());
  });
});

describe('for a reader', () => {
  it('offers the preview and the save to the owner, and neither to a reader', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    const offered = () =>
      ['Preview impact', 'Save rule'].filter((name) => screen.queryByRole('button', { name }));
    render();
    await screen.findByLabelText('Rule name');
    expect(offered()).toEqual(['Preview impact', 'Save rule']);
    cleanup();

    publishAccess(true);
    render();

    await screen.findByLabelText('Rule name');
    expect(offered()).toEqual([]);
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
  });
});
