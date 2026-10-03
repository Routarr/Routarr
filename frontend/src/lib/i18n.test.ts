import { afterEach, describe, it, expect, vi } from 'vitest';

import { api } from '../api/client';
import { loadDictionary, seedDictionary, t } from './i18n.svelte';

describe('t', () => {
  /**
   * A value is text the reader typed or the library holds, a rule named after
   * a placeholder among them. Replaced in turn by the parameter after it, the
   * name would read as a count.
   */
  it('never replaces a placeholder that arrived inside a value', () => {
    seedDictionary({ RuleHolds: '{name} holds {count}' });

    expect(t('RuleHolds', { name: 'Rule {count}', count: 3 })).toBe('Rule {count} holds 3');
  });
});

describe('loadDictionary', () => {
  afterEach(() => vi.restoreAllMocks());

  /** `nb_NO` is a dictionary's name, not a language tag a browser reads. */
  it('marks the page with a language tag the browser reads', async () => {
    vi.spyOn(api, 'getLocalization').mockResolvedValue({
      language: 'nb_NO',
      direction: 'ltr',
      strings: {},
    } as Awaited<ReturnType<typeof api.getLocalization>>);

    await loadDictionary();

    expect(document.documentElement.lang).toBe('nb-NO');
  });
});

describe('a count in a sentence', () => {
  /** Written as the language groups digits, as every size and percentage is. */
  it('is grouped the way the language writes numbers, and a year is not', () => {
    seedDictionary({ Tasks: 'Tasks: {count}', Since: 'Since {min}' }, 'fr');

    expect(t('Tasks', { count: 12345 })).toBe('Tasks: 12 345');
    expect(t('Since', { min: 2026 })).toBe('Since 2026');
  });
});
