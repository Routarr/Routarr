import { describe, it, expect, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { navigate } from '../lib/router.svelte';
import { withBase } from '../test/base';
import NotFound from './NotFound.svelte';

afterEach(() => withBase(null));

describe('NotFound', () => {
  it('names the address that reached nothing', () => {
    navigate('/typo');
    renderWithI18n(NotFound);

    expect(screen.getByRole('heading', { name: 'Page not found', level: 1 })).toBeInTheDocument();
    // The path, not a generic apology: it is what tells the reader whether they
    // mistyped or followed a link that has moved.
    expect(screen.getByText('Routarr serves nothing at /typo.')).toBeInTheDocument();
  });

  it('offers a way out that is a link', () => {
    renderWithI18n(NotFound);

    expect(screen.getByRole('link', { name: 'Back to the dashboard' })).toHaveAttribute(
      'href',
      '/',
    );
  });

  it('points the way out at the mount point, not at the proxy root', () => {
    withBase('/routarr/');
    renderWithI18n(NotFound);

    expect(screen.getByRole('link', { name: 'Back to the dashboard' })).toHaveAttribute(
      'href',
      '/routarr/',
    );
  });
});
