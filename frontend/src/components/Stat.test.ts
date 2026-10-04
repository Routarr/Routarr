import { describe, it, expect } from 'vitest';

import { renderWithI18n } from '../test/render';
import Stat from './Stat.svelte';

/** A figure and its name, on the strip of figures several screens draw. */
describe('Stat', () => {
  /** Nothing failed is not a failure: a zero painted red reads as one. */
  it('draws a zero without its tone, and any other figure with it', () => {
    const zero = renderWithI18n(Stat, { props: { label: 'Failed', value: 0, tone: 'danger' } });
    const three = renderWithI18n(Stat, { props: { label: 'Failed', value: 3, tone: 'danger' } });

    expect(zero.container.querySelector('.metric')?.className).not.toContain('is-danger');
    expect(three.container.querySelector('.metric')?.className).toContain('is-danger');
  });

  /** `12345 titles evaluated` reads as a code, `12 345` as the count it is. */
  it('groups the figure the way the language writes numbers', () => {
    const { container } = renderWithI18n(Stat, {
      props: { label: 'Evaluated', value: 12345 },
      language: 'fr',
    });

    expect(container.querySelector('.metric-value')?.textContent).toBe('12\u202f345');
  });
});
