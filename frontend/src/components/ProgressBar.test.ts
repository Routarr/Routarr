import { describe, it, expect } from 'vitest';
import { screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import ProgressBar from './ProgressBar.svelte';

describe('ProgressBar', () => {
  /** Read by a screen reader as how far, and by the eye as a bar and two figures. */
  it('says how far a task has gone, its figures grouped as the language writes them', () => {
    renderWithI18n(ProgressBar, {
      props: { current: 1500, total: 12000, label: 'Simulation' },
      language: 'de',
    });

    const bar = screen.getByRole('progressbar', { name: 'Simulation' });
    expect(bar).toHaveAttribute('aria-valuenow', '1500');
    expect(bar).toHaveAttribute('aria-valuemax', '12000');
    expect(screen.getByText('1.500/12.000')).toBeTruthy();
  });

  /** A count past its total, as a task that found more than it expected, fills the bar and no more. */
  it('stops at a full bar', () => {
    const { container } = renderWithI18n(ProgressBar, {
      props: { current: 12, total: 10, label: 'Sync' },
    });

    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '10');
    expect((container.querySelector('.progress-fill') as HTMLElement).style.width).toBe('100%');
  });
});
