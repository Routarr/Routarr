import { describe, it, expect } from 'vitest';

import { seedDictionary, t } from './i18n.svelte';

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
