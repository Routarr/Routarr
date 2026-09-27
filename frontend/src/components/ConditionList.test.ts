import { describe, it, expect, vi } from 'vitest';
import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import type { Condition, ConditionSpec } from '../api/types';
import ConditionList from './ConditionList.svelte';

/**
 * The picker offers what the rule can use; the list renders what the rule
 * already has. Those are two different sets, and conflating them is how a
 * saved condition disappears the next time somebody opens the rule.
 */

const STRINGS = {
  AddCondition: 'Add a condition',
  ConditionsAllOf: 'All of',
  NeedsMetadataSuffix: '(needs metadata)',
  Remove: 'Remove',
  QuantifierAny: 'any of',
  QuantifierAll: 'all of',
  QuantifierLabel: 'How these values combine',
  PlaceholderStringList: 'comma separated',
  PlaceholderNumber: 'number',
};

const spec = (type: string, media_types: string[], label: string): ConditionSpec => ({
  type,
  label,
  value_type: type === 'season_count_over' ? 'number' : 'string_list',
  needs_metadata: false,
  metadata_field: null,
  available: true,
  media_types,
  suggestions: '',
  quantifier: '',
  counterpart: '',
});

const GENRE: ConditionSpec = {
  ...spec('genre_contains', ['movie', 'series'], 'Genre contains'),
  quantifier: 'any',
  counterpart: 'genre_contains_all',
};
const GENRE_ALL: ConditionSpec = {
  ...spec('genre_contains_all', ['movie', 'series'], 'Genre contains all of'),
  quantifier: 'all',
  counterpart: 'genre_contains',
};
const SEASONS = spec('season_count_over', ['series'], 'Season count over');

const LANGUAGE: ConditionSpec = {
  ...spec('original_language', ['movie', 'series'], 'Original language is'),
  suggestions: 'original_languages',
};

function render(
  conditions: Condition[],
  addable: ConditionSpec[],
  onRetype: (index: number, type: string) => void = () => {},
  facets?: unknown,
  language = 'en',
) {
  renderWithI18n(ConditionList, {
    language,
    props: {
      title: 'All of',
      list: 'conditions',
      conditions,
      specs: [GENRE, GENRE_ALL, SEASONS, LANGUAGE],
      addable,
      facets,
      onAdd: () => {},
      onRetype,
      onUpdate: () => {},
      onRemove: () => {},
    },
    strings: STRINGS,
  });
}

describe('ConditionList', () => {
  it('offers only the conditions the rule can use', () => {
    render([], [GENRE]);

    const picker = screen.getByRole('combobox', { name: 'All of' });
    const offered = [...picker.querySelectorAll('option')].map((o) => o.textContent?.trim());

    expect(offered).toContain('Genre contains');
    expect(offered).not.toContain('Season count over');
  });

  it('still renders a condition the rule already carries but could not add today', () => {
    // A rule saved as `series` and later narrowed to `movie` keeps its season
    // condition. Hiding it would drop it from the payload on the next save —
    // the user would never see what they lost.
    render([{ type: 'season_count_over', value: 3 } as unknown as Condition], [GENRE]);

    expect(screen.getByText('Season count over')).toBeTruthy();
  });

  /**
   * OR and AND are one selector on the condition, not the difference between
   * one condition and two. Both halves of the pair are the same question, so
   * only one of them is offered when adding.
   */
  it('offers each quantified condition once', () => {
    render([], [GENRE, GENRE_ALL]);

    const picker = screen.getByRole('combobox', { name: 'All of' });
    const offered = [...picker.querySelectorAll('option')].map((o) => o.textContent?.trim());
    expect(offered).toEqual(['Add a condition', 'Genre contains']);
  });

  it('captions both halves of a pair alike, the selector carrying the difference', () => {
    render([{ type: 'genre_contains_all', value: ['Animation'] }], [GENRE]);

    // Scoped to the row: the add picker legitimately offers an option under the
    // same name, and a page-wide query matches it too.
    const row = document.querySelector('.condition-row');
    expect(row?.textContent).toContain('Genre contains');
    expect(row?.textContent).not.toContain('Genre contains all of');
    expect(screen.getByRole('combobox', { name: /How these values combine/ })).toHaveValue('all');
  });

  it('swaps the condition for its counterpart, keeping the values', async () => {
    const onRetype = vi.fn();
    render([{ type: 'genre_contains', value: ['Animation', 'Family'] }], [GENRE], onRetype);

    await userEvent.selectOptions(
      screen.getByRole('combobox', { name: /How these values combine/ }),
      'all',
    );
    expect(onRetype).toHaveBeenCalledWith(0, 'genre_contains_all');
  });

  it('leaves a condition with no quantifier without a selector', () => {
    render([{ type: 'season_count_over', value: 3 }], [SEASONS]);

    expect(screen.queryByRole('combobox', { name: /How these values combine/ })).toBeNull();
  });

  /**
   * A title has one original language, so several values on this condition can
   * only be alternatives. Unsaid, the search box after the first one reads as
   * an invitation to give a title a second language.
   */
  it('says in words that the values of a single-valued axis are alternatives', () => {
    render([{ type: 'original_language', value: ['ja'] }], [LANGUAGE]);

    expect(screen.queryByRole('combobox', { name: /How these values combine/ })).toBeNull();
    expect(screen.getByText('any of')).toBeTruthy();
  });

  /** Heard with the field, not only seen beside it. */
  it('ties the fixed "any of" to the field it qualifies', () => {
    render([{ type: 'original_language', value: ['ja'] }], [LANGUAGE]);

    expect(screen.getByRole('combobox', { name: LANGUAGE.label })).toHaveAccessibleDescription(
      'any of',
    );
  });

  it('says nothing about combining a value that is not a list', () => {
    render([{ type: 'season_count_over', value: 3 }], [SEASONS]);

    expect(screen.queryByText('any of')).toBeNull();
  });
  /**
   * The library counts values, the closed vocabulary names them, and a value in
   * both needs both.
   *
   * Appended rather than merged, the five codes actually synced were the only
   * ones shown bare — `en` and `fr` above a list of `Afrikaans (af)` — which
   * reads as the known languages being the ones nobody bothered to name.
   */
  it('names a value the library holds from the vocabulary that defines it', async () => {
    render([{ type: 'original_language', value: [] } as unknown as Condition], [], () => {}, {
      vocabularies: {
        original_languages: [
          { value: 'en', label: 'English (en)', count: 0 },
          { value: 'af', label: 'Afrikaans (af)', count: 0 },
        ],
        origin_countries: [],
      },
      original_languages: [{ value: 'en', count: 32 }],
    });

    await userEvent.click(screen.getByRole('combobox', { name: 'Original language is' }));

    // The one the library holds keeps its count *and* gains its name.
    expect(screen.getByRole('option', { name: /English \(en\)/ })).toBeTruthy();
    expect(screen.getByRole('option', { name: /Afrikaans \(af\)/ })).toBeTruthy();
  });

  /** Chosen from names in the reader's language, stored as the code. */
  it('offers a language by its name in the language of the interface', async () => {
    render(
      [{ type: 'original_language', value: [] } as unknown as Condition],
      [],
      () => {},
      {
        vocabularies: {
          original_languages: [{ value: 'ja', label: 'Japanese (ja)', count: 0 }],
          origin_countries: [],
        },
        original_languages: [],
      },
      'fr',
    );

    await userEvent.click(screen.getByRole('combobox', { name: 'Original language is' }));

    expect(screen.getByRole('option', { name: 'japonais (ja)' })).toBeTruthy();
  });
});

/**
 * Delete takes its own row away, and the focus with it, inside the rule
 * editor's dialog. Rows are kept by position, so the condition
 * after it moves up under the focus. The last row takes its button with it,
 * and the picker below is what is left.
 */
describe('deleting a condition', () => {
  function renderLive(conditions: Condition[]) {
    const view = renderWithI18n(ConditionList, {
      props: {
        title: 'All of',
        list: 'conditions',
        conditions,
        specs: [GENRE, GENRE_ALL, SEASONS, LANGUAGE],
        addable: [GENRE],
        onAdd: () => {},
        onRetype: () => {},
        onUpdate: () => {},
        onRemove: (index: number) =>
          void view.rerender({ conditions: conditions.filter((_, at) => at !== index) }),
      },
      strings: STRINGS,
    });
  }

  it('leaves the focus on the Delete of the condition that took its place', async () => {
    renderLive([
      { type: 'genre_contains', value: ['Anime'] },
      { type: 'season_count_over', value: 3 },
    ]);

    await userEvent.click(screen.getByRole('button', { name: 'Delete – Genre contains' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Delete – Season count over' }),
      ),
    );
  });

  it('hands the focus to the picker once the last condition is deleted', async () => {
    renderLive([{ type: 'genre_contains', value: ['Anime'] }]);

    await userEvent.click(screen.getByRole('button', { name: 'Delete – Genre contains' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('combobox', { name: 'All of' })),
    );
  });
});
