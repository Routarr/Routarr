import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { api } from '../api/client';
import type { RuleDraft } from '../api/types';
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
  it('names a dead condition before the rule is saved, and holds Save until it is fixed', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [{ severity: 'error', field: 'conditions', message: 'Condition #1 has no value' }],
    });
    render();
    await touch();

    await waitFor(() => expect(screen.getByText('Condition #1 has no value')).toBeInTheDocument());
    expect(screen.getByRole('list', { name: 'Validation results' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeDisabled();
  });

  it('shows a warning without holding Save', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: true,
      issues: [{ severity: 'warning', field: 'target_category', message: 'anime is not mapped' }],
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
    // count is what pins it — a test that never types cannot fail for the
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
      issues: [{ severity: 'error', field: 'name', message: 'A rule needs a name' }],
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
      issues: [{ severity: 'error', field: 'name', message: 'A rule needs a name' }],
    });
    render();

    // The question is still asked — the answer is simply not thrown at anyone.
    await waitFor(() => expect(validate).toHaveBeenCalled());
    expect(screen.queryByText('A rule needs a name')).toBeNull();
    // And Save is not dead: a button disabled by a reason nobody is shown is
    // worse than the premature complaint.
    expect(screen.getByRole('button', { name: 'Save rule' })).toBeEnabled();

    await touch();
    await waitFor(() => expect(screen.getByText('A rule needs a name')).toBeInTheDocument());
  });

  /// An existing rule was saved once, so an issue on it is news about something
  /// that changed underneath — a category deleted since, say — and it is said
  /// on open.
  it('speaks immediately about a rule that already exists', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [{ severity: 'error', field: 'target_category', message: 'anime no longer exists' }],
    });
    render('rule-1');

    await waitFor(() => expect(screen.getByText('anime no longer exists')).toBeInTheDocument());
  });

  /**
   * The editor's own verdict is the one that speaks.
   *
   * The browser's native bubble renders in the *browser's* language whatever
   * `ui_language` says, and fires before the submit handler — so it speaks over
   * the translated list below. jsdom runs no constraint validation at all, so
   * nothing but this assertion can see the decision.
   */
  it('keeps the browser out of the conversation', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({ valid: true, issues: [] });
    // Through the helper, which is the only place `getLibraryFacets` is
    // stubbed: rendering directly sent a real request from jsdom.
    render();

    const form = document.querySelector('form');
    expect(form?.hasAttribute('novalidate')).toBe(true);
    // `required` stays: it is what says the field is mandatory to anything
    // reading the form, and only the bubble was in the way.
    expect(document.querySelector('#rules-rule-name')?.hasAttribute('required')).toBe(true);
  });

  /// Pressing Save on an untouched form is also asking: the answer appears
  /// there rather than through a round trip the server would only refuse.
  it('answers the first Save press instead of sending a draft it knows is refused', async () => {
    vi.spyOn(api, 'validateRule').mockResolvedValue({
      valid: false,
      issues: [{ severity: 'error', field: 'name', message: 'A rule needs a name' }],
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
 * `Rules` already holds the facets for its panel; the editor asking again
 * aggregated the whole library a second time every time it opened.
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
