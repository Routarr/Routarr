import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';

import SearchFieldHarness from '../test/SearchFieldHarness.svelte';

/**
 * A screen whose `deps` read the bound value fetches on every change of it,
 * so typing "anime" was five `LIKE` queries against a homelab server. The
 * field owns the keystrokes and hands the value over once the typing stops.
 */

afterEach(() => vi.useRealTimers());

async function type(word: string) {
  const input = screen.getByRole('searchbox', { name: 'Search' });
  for (let i = 1; i <= word.length; i += 1) {
    await fireEvent.input(input, { target: { value: word.slice(0, i) } });
  }
}

describe('SearchField', () => {
  it('hands the value over once per word rather than once per letter', async () => {
    vi.useFakeTimers();
    render(SearchFieldHarness, { debounce: 200 });

    await type('anime');
    expect(screen.getByTestId('bound')).toHaveTextContent('');

    await vi.advanceTimersByTimeAsync(200);
    expect(screen.getByTestId('bound')).toHaveTextContent('anime');
  });

  it('hands it over at once on Enter, so a submit reads what was typed', async () => {
    vi.useFakeTimers();
    render(SearchFieldHarness, { debounce: 200 });

    await type('akira');
    await fireEvent.keyDown(screen.getByRole('searchbox', { name: 'Search' }), { key: 'Enter' });

    expect(screen.getByTestId('bound')).toHaveTextContent('akira');
  });

  it('binds on every keystroke where no delay is asked for', async () => {
    render(SearchFieldHarness);

    await type('ak');

    expect(screen.getByTestId('bound')).toHaveTextContent('ak');
  });
});
