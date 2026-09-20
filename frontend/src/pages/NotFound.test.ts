import { describe, it, expect, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { navigate } from '../lib/router.svelte';
import { withBase } from '../test/base';
import NotFound from './NotFound.svelte';

/**
 * The screen a typed address or an old bookmark lands on. Its whole job is to
 * say where you are, since showing the dashboard said nothing at all.
 */

const STRINGS = {
  NotFoundTitle: 'Page not found',
  NotFoundSubtitle: 'Routarr serves nothing at {path}.',
  NotFoundHelp: 'Check the address, or pick a screen from the menu.',
  BackToDashboard: 'Back to the dashboard',
};

afterEach(() => {
  withBase(null);
  navigate('/');
});

describe('NotFound', () => {
  it('names the address that reached nothing', () => {
    navigate('/typo');
    renderWithI18n(NotFound, { strings: STRINGS });

    expect(screen.getByRole('heading', { name: 'Page not found', level: 1 })).toBeInTheDocument();
    // The path, not a generic apology: it is what tells the reader whether they
    // mistyped or followed a link that has moved.
    expect(screen.getByText('Routarr serves nothing at /typo.')).toBeInTheDocument();
  });

  /// A real anchor, so middle-click and the status bar work and the delegated
  /// listener in the router intercepts it like every other internal link.
  it('offers a way out that is a link', () => {
    renderWithI18n(NotFound, { strings: STRINGS });

    expect(screen.getByRole('link', { name: 'Back to the dashboard' })).toHaveAttribute(
      'href',
      '/',
    );
  });

  it('points the way out at the mount point, not at the proxy root', () => {
    withBase('/routarr/');
    renderWithI18n(NotFound, { strings: STRINGS });

    expect(screen.getByRole('link', { name: 'Back to the dashboard' })).toHaveAttribute(
      'href',
      '/routarr/',
    );
  });
});
