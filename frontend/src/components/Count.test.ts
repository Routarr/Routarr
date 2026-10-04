import { describe, it, expect } from 'vitest';
import { screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import Count from './Count.svelte';

describe('Count', () => {
  /** One shape on every screen that counts folders, grouped as the language groups digits. */
  it('draws a count as a number, grouped the way the language writes it', () => {
    renderWithI18n(Count, { props: { value: 12345 }, language: 'de' });

    const count = screen.getByText('12.345');
    expect(count).toHaveClass('num');
  });

  it('draws zero like any other count', () => {
    renderWithI18n(Count, { props: { value: 0 } });

    expect(screen.getByText('0')).toHaveClass('num');
  });
});
