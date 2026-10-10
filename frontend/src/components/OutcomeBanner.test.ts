import { describe, it, expect } from 'vitest';
import { screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';

import { renderWithI18n } from '../test/render';
import { createOutcome } from '../lib/outcome.svelte';
import OutcomeBanner from './OutcomeBanner.svelte';

/**
 * A live region added at the same time as its text announces nothing: a screen
 * reader hears every refusal, which is an alert, and no success. The region a
 * success or a partial result is said in has to be there before either.
 */
describe('OutcomeBanner', () => {
  it('holds its polite region before any outcome, and says each one in it', () => {
    const outcome = createOutcome();
    renderWithI18n(OutcomeBanner, { props: { outcome }, strings: {} });

    const region = screen.getByRole('status');
    expect(region).toBeEmptyDOMElement();

    outcome.succeed('Settings saved');
    flushSync();
    expect(region).toHaveTextContent('Settings saved');

    outcome.warn('Instances synced: 1', ['Sonarr: the key was refused']);
    flushSync();
    expect(region).toHaveTextContent('Instances synced: 1');
    expect(region).not.toHaveTextContent('Settings saved');
  });

  /**
   * Forty failed moves are one failure with forty lines, not forty alerts a
   * screen reader reads one after the other.
   */
  it('lists the failed items in one banner, under the one alert', () => {
    const outcome = createOutcome();
    renderWithI18n(OutcomeBanner, { props: { outcome }, strings: {} });

    outcome.fail('Applied: 0 of 3', ['Akira: refused', 'Dune: refused', 'Heat: refused']);
    flushSync();

    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getAllByRole('listitem').map((item) => item.textContent)).toEqual([
      'Akira: refused',
      'Dune: refused',
      'Heat: refused',
    ]);
  });
});
