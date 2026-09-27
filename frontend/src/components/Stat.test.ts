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
});
