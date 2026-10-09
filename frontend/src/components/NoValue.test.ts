import { describe, it, expect } from 'vitest';

import { renderWithI18n } from '../test/render';
import NoValue from './NoValue.svelte';

describe('NoValue', () => {
  /** A dash on screen, "none" to a screen reader, and never "dash". */
  it('shows a dash and says a word', () => {
    const { container } = renderWithI18n(NoValue, {
      strings: { None: '-', NoneSpoken: 'none' },
    });

    expect(container.querySelector('[aria-hidden="true"]')?.textContent).toBe('-');
    expect(container.querySelector('.visually-hidden')?.textContent).toBe('none');
  });
});
