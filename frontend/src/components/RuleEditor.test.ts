import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { nthCall } from '../test/spy';
import { ApiError, api } from '../api/client';
import type { PreviewChange, RuleDraft, SimulationSummary } from '../api/types';
import RuleEditor from './RuleEditor.svelte';

/**
 * The editor asks the server what it thinks of the draft as it is typed, so a
 * condition that can never match is named before Save is pressed rather than
 * discovered as a rule that silently routes nothing.
 */

const STRINGS = {
  CreateRule: 'Create rule',
  SaveRule: 'Save rule',
  ValidationIssues: 'Validation results',
  RuleName: 'Rule name',
  Priority: 'Priority',
  EnterWholeNumber: 'Enter a whole number.',
  PreviewImpact: 'Preview impact',
  PreviewTitle: 'Impact preview',
  PreviewSummary: 'Changing: {changed}. Moves from {beforeMoves} to {afterMoves}.',
  PreviewTruncated: 'Shown: {shown} of {total}.',
};

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

/** Any edit at all: what turns an untouched form into one being built. */
async function touch() {
  await userEvent.type(await screen.findByLabelText('Rule name'), '!');
}

function render(ruleId?: string, extra: Record<string, unknown> = {}) {
  vi.spyOn(api, 'getLibraryFacets').mockResolvedValue({
    total_media: 0,
    without_metadata: 0,
    vocabularies: { original_languages: [], origin_countries: [] },
    genres: [],
    original_languages: [],
    origin_countries: [],
    certifications: [],
    tags: [],
    series_types: [],
    root_folders: [],
  });
  renderWithI18n(RuleEditor, {
    props: {
      draft: DRAFT,
      ruleId,
      categories: [],
      catalog: { conditions: [] },
      onClose: () => {},
      onSaved: async () => {},
      ...extra,
    },
    strings: STRINGS,
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
      expect(screen.getByText('Condition 1 (Year): the range is inverted')).toBeInTheDocument(),
    );
    expect(screen.getByRole('list', { name: 'Validation results' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeDisabled();
  });

  /**
   * A priority is a whole number. Sent to the server, `1.5` is refused in
   * English inside a 422 the live check swallows, and Save would stay lit on a
   * rule that cannot be saved.
   */
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

  it('asks the server once the draft settles, not on every keystroke', async () => {
    const validate = vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    render();
    await waitFor(() => expect(validate).toHaveBeenCalledTimes(1));

    // Four keystrokes in a row. Without the debounce each one asks, so the
    // count is what pins it: a test that never types cannot fail for the
    // reason its name gives.
    await userEvent.type(await screen.findByLabelText('Rule name'), 'four');

    await waitFor(() => expect(validate).toHaveBeenCalledTimes(2));
    expect(validate.mock.lastCall?.[0]).toMatchObject({ name: 'Anime' + 'four' });
  });

  /**
   * The latch does not open again.
   *
   * Compared against the opening draft instead, adding a condition and removing
   * it makes the form untouched a second time: the verdict vanishes and Save
   * lights up on a rule with no name and no condition.
   */
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
    await waitFor(() => expect(screen.getByText('A rule needs a name')).toBeInTheDocument());

    await userEvent.clear(field);
    await userEvent.type(field, 'Anime');

    expect(screen.getByText('A rule needs a name')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeDisabled();
  });

  /**
   * A form nobody has touched is not wrong yet: both complaints a new rule
   * draws are about a form that is merely empty, and the empty fields say so
   * already.
   */
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
    // And Save is not dead: a button disabled by a reason nobody is shown is
    // worse than the premature complaint.
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeEnabled();

    await touch();
    await waitFor(() => expect(screen.getByText('A rule needs a name')).toBeInTheDocument());
  });

  /**
   * An existing rule was saved once, so an issue on it is news about something
   * that changed underneath (a category deleted since, say), and it is said
   * on open.
   */
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

    await waitFor(() => expect(screen.getByText('anime no longer exists')).toBeInTheDocument());
  });

  /**
   * The form is `novalidate` (every form is, `layout.test.ts` holds it), so
   * the browser's bubble never speaks over the translated verdict. `required`
   * stays: it is what says the field is mandatory to anything reading the form.
   */
  it('keeps the name marked required', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    // Through the helper, which is the only place `getLibraryFacets` is
    // stubbed: rendering directly sends a real request from jsdom.
    render();

    expect(document.querySelector('#rules-rule-name')?.hasAttribute('required')).toBe(true);
  });

  /**
   * A condition just added has no value yet. Flagged before the reader had a
   * chance to pick one, it reads as a mistake they did not make, and it holds
   * Save for a reason that is only the order of their clicks.
   */
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

    expect(
      await screen.findByText('Condition 1 (Genre contains) has no value'),
    ).toBeInTheDocument();
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

  /**
   * Pressing Save on an untouched form is also asking: the answer appears
   * there rather than through a round trip the server would only refuse.
   */
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

    await fireEvent.click(screen.getByRole('button', { name: 'Save rule' }));

    expect(await screen.findByText('A rule needs a name')).toBeInTheDocument();
    expect(create).not.toHaveBeenCalled();
  });
});

/**
 * `Rules` already holds the facets for its panel, and the editor asking again
 * would aggregate the whole library a second time every time it opens.
 */
describe('the facets the parent already holds', () => {
  it('are used as they are, without a second request', async () => {
    render(undefined, {
      knownFacets: {
        total_media: 3,
        without_metadata: 0,
        vocabularies: { original_languages: [], origin_countries: [] },
        genres: [{ value: 'Animation', count: 3 }],
        original_languages: [],
        origin_countries: [],
        certifications: [],
        tags: [],
        series_types: [],
        root_folders: [],
      },
    });
    await screen.findByLabelText('Rule name');

    expect(api.getLibraryFacets).not.toHaveBeenCalled();
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

  /**
   * A rule is understood by what it moves. The draft is asked about as it
   * stands, and nothing is saved by asking: the panel says so in its title.
   */
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

    expect(await screen.findByText('Changing: 1. Moves from 2 to 3.')).toBeTruthy();
    const row = screen.getByRole('row', { name: /Akira/ });
    expect(row).toHaveTextContent(/films\s*anime/);
    expect(nthCall(preview)).toEqual([expect.objectContaining({ name: 'Anime' }), 'r1']);
    expect(screen.queryByText(/Shown:/)).toBeNull();
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

    expect(await screen.findByText('Shown: 1 of 38.')).toBeTruthy();
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
});
