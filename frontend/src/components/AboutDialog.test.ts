import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api } from '../api/client';
import AboutDialog from './AboutDialog.svelte';

/**
 * What Routarr is: its version, its licence and where its source lives, the
 * licences of what it is built with, and its sources' credits.
 */

afterEach(() => vi.restoreAllMocks());

describe('AboutDialog', () => {
  it('names the version and the licences, credits the sources, and closes', async () => {
    vi.spyOn(api, 'getMetadataProviders').mockResolvedValue({ providers: [], order: [] });
    const onClose = vi.fn();
    renderWithI18n(AboutDialog, { props: { version: '0.2.0', onClose } });

    expect(screen.getByRole('dialog', { name: 'About Routarr' })).toBeTruthy();
    expect(screen.getByText('Version 0.2.0')).toBeTruthy();
    expect(
      screen.getByText(/^Free software under the GNU General Public License, version 3/),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'github.com/Routarr/Routarr' })).toHaveAttribute(
      'href',
      'https://github.com/Routarr/Routarr',
    );
    // Relative, so the `<base href>` puts it under the mount point.
    expect(screen.getByRole('link', { name: 'Third-party licences' })).toHaveAttribute(
      'href',
      'third-party-notices.txt',
    );
    expect(await screen.findByRole('heading', { name: 'Data sources' })).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
