import { describe, it, expect, vi } from 'vitest';
import { screen } from '@testing-library/svelte';
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
) {
  renderWithI18n(ConditionList, {
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
});
