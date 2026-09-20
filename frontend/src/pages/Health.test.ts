import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { health, healthInstance } from '../test/fixtures';
import { api } from '../api/client';
import type { Health } from '../api/types';
import HealthPage from './Health.svelte';

/**
 * Diagnostics is where a homelab operator finds out why nothing is moving. It
 * has one job: report what is wrong without inventing a verdict of its own.
 */

const STRINGS = {
  Diagnostics: 'Diagnostics',
  AllGood: 'Everything checks out.',
  NoInstanceConfigured: 'No instance configured',
  Connected: 'connected',
  Disabled: 'disabled',
  Error: 'error',
  ProviderNeedsKey: 'needs an API key',
  ProviderActive: 'active',
  SettingMetadataProviders: 'Metadata sources',
  ArrInstances: 'Arr instances',
  Never: 'never',
  None: '—',
};

const show = () => renderWithI18n(HealthPage, { strings: STRINGS });

const withProviders = (list: Health['metadata']['providers']) =>
  health({ metadata: { providers: list, cached_items: 0, media_missing_metadata: 0 } });

afterEach(() => vi.restoreAllMocks());

describe('Diagnostics', () => {
  it('says everything checks out rather than leaving the page silent', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    show();

    expect(await screen.findByText('Everything checks out.')).toBeTruthy();
  });

  it('lists every warning it was given, not a sample of them', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ warnings: ['one', 'two', 'three', 'four', 'five'] }),
    );
    const { container } = show();

    await screen.findByText('one');
    // The dashboard caps at three because it is a summary; this page is the
    // full list, and a cap here would hide the problem it exists to report.
    expect(container.querySelectorAll('.banner-warning')).toHaveLength(5);
    expect(screen.getByText('five')).toBeTruthy();
    expect(screen.queryByText('Everything checks out.')).toBeNull();
  });

  /**
   * A source waiting for a key is not a broken source, and a source that failed
   * its probe is not one waiting for a key. They must not read the same.
   */
  it('tells a source waiting for a key apart from one that answered badly', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      withProviders([
        { id: 'tmdb', display_name: 'TMDb', needs_key: true, configured: false, connected: null },
        { id: 'omdb', display_name: 'OMDb', needs_key: true, configured: true, connected: false },
        {
          id: 'arr',
          display_name: 'Radarr / Sonarr',
          needs_key: false,
          configured: true,
          connected: true,
        },
      ]),
    );
    const { container } = show();

    await screen.findByText('TMDb');
    const rows = [...container.querySelectorAll('.source-row')];
    expect(within(rows[0] as HTMLElement).getByText('needs an API key')).toBeTruthy();
    expect(within(rows[1] as HTMLElement).getByText('error')).toBeTruthy();
    expect(within(rows[2] as HTMLElement).getByText('connected')).toBeTruthy();
  });

  /**
   * A source that is configured and was never probed is active, not connected:
   * `arr` answers from the row the sync already wrote and makes no request, so
   * claiming a connection would be claiming something never checked.
   */
  it('calls an unprobed source active rather than connected', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      withProviders([
        {
          id: 'arr',
          display_name: 'Radarr / Sonarr',
          needs_key: false,
          configured: true,
          connected: null,
        },
      ]),
    );
    const { container } = show();

    await screen.findByText('active');
    // Scoped to the source list: the footer's database badge legitimately says
    // "connected", so a page-wide assertion matches it instead.
    const row = container.querySelector('.source-row') as HTMLElement;
    expect(within(row).getByText('active')).toBeTruthy();
    expect(within(row).queryByText('connected')).toBeNull();
  });

  it('reports an instance failure in the words the server used', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ instances: [healthInstance({ status: 'connection refused', version: null })] }),
    );
    show();

    expect(await screen.findByText('connection refused')).toBeTruthy();
  });

  /**
   * Zero mapped folders is already in the warnings above. Painting the number
   * red as well makes the table look like it has found a second, different
   * problem — it is a count, not a verdict.
   */
  it('reports mapped folders as a count, without a verdict of its own', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ instances: [healthInstance({ mapped_root_folders: 0 })] }),
    );
    const { container } = show();

    await screen.findByText('Radarr');
    const cell = container.querySelector('.num') as HTMLElement;
    expect(cell.textContent).toBe('0');
    expect(cell.className).not.toContain('danger');
  });

  it('invites a first instance instead of showing an empty table', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    show();

    expect(await screen.findByText('No instance configured')).toBeTruthy();
  });
});
