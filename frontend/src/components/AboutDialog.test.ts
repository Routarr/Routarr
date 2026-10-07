import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api } from '../api/client';
import AboutDialog from './AboutDialog.svelte';

/** What Routarr is: its version, its licence and where its source lives, and its sources' credits. */

const STRINGS = {
  AboutRoutarr: 'About Routarr',
  AboutVersion: 'Version {version}',
  AboutLicence: 'Free software under the GPL, its source code at',
  SourceCreditsTitle: 'Data sources',
  Dismiss: 'Close',
};

afterEach(() => vi.restoreAllMocks());

describe('AboutDialog', () => {
  it('names the version and the licence, credits the sources, and closes', async () => {
    vi.spyOn(api, 'getMetadataProviders').mockResolvedValue({ providers: [], order: [] });
    const onClose = vi.fn();
    renderWithI18n(AboutDialog, { props: { version: '0.2.0', onClose }, strings: STRINGS });

    expect(screen.getByRole('dialog', { name: 'About Routarr' })).toBeTruthy();
    expect(screen.getByText('Version 0.2.0')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'github.com/Routarr/Routarr' })).toHaveAttribute(
      'href',
      'https://github.com/Routarr/Routarr',
    );
    expect(await screen.findByRole('heading', { name: 'Data sources' })).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
