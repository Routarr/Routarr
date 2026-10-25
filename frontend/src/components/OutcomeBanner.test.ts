import { describe, it, expect } from 'vitest';
import { screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';

import { renderWithI18n } from '../test/render';
import { createOutcome } from '../lib/outcome.svelte';
import OutcomeBanner from './OutcomeBanner.svelte';

describe('OutcomeBanner', () => {
  it('holds its polite region before any outcome, and says each one in it', () => {
    const outcome = createOutcome();
    renderWithI18n(OutcomeBanner, { props: { outcome } });

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

  it('lists the failed items in one banner, under the one alert', () => {
    const outcome = createOutcome();
    renderWithI18n(OutcomeBanner, { props: { outcome } });

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
