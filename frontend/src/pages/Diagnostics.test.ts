import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { health, healthInstance, warning } from '../test/fixtures';
import { api } from '../api/client';
import type { Health } from '../api/types';
import { statusRevision } from '../lib/status.svelte';
import Diagnostics from './Diagnostics.svelte';

const show = () => renderWithI18n(Diagnostics);

const withProviders = (list: Health['metadata']['providers']) =>
  health({ metadata: { providers: list, cached_items: 0, media_missing_metadata: 0 } });

afterEach(() => vi.restoreAllMocks());

/**
 * Diagnostics is where a homelab operator finds out why nothing is moving. It
 * has one job: report what is wrong without inventing a verdict of its own.
 */
describe('Diagnostics', () => {
  /** A re-check waits out every Arr that does not answer, and says it is running. */
  it('says the re-check is running until it ends', async () => {
    vi.spyOn(api, 'getHealth')
      .mockResolvedValueOnce(health())
      .mockReturnValue(new Promise(() => {}));
    show();
    const recheck = await screen.findByRole('button', { name: 'Re-check' });
    await waitFor(() => expect(recheck).not.toHaveAttribute('aria-busy', 'true'));

    await fireEvent.click(recheck);

    expect(recheck).toHaveAttribute('aria-busy', 'true');
  });

  /** Opening the screen reads a probe of the last moments again, a recheck asks anew. */
  it('asks for a fresh probe on a recheck alone', async () => {
    const getHealth = vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    show();
    const recheck = await screen.findByRole('button', { name: 'Re-check' });
    await waitFor(() => expect(getHealth).toHaveBeenCalledTimes(1));
    expect(getHealth).toHaveBeenLastCalledWith({ fresh: false }, expect.any(AbortSignal));

    await fireEvent.click(recheck);

    await waitFor(() => expect(getHealth).toHaveBeenCalledTimes(2));
    expect(getHealth).toHaveBeenLastCalledWith({ fresh: true }, expect.any(AbortSignal));
  });

  it('says everything checks out rather than leaving the page silent', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    show();

    expect(await screen.findByText('Everything checks out.')).toBeTruthy();
  });

  /**
   * A probe records what it found, and the shell counts that among its
   * warnings. Unannounced, the badge leaves out an unreachable Arr this screen
   * lists, until the next poll a minute later.
   */
  it('has the shell count again what a probe found', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    const before = statusRevision();
    show();

    await screen.findByText('Everything checks out.');
    expect(statusRevision()).toBeGreaterThan(before);
  });

  /** One banner, as the dashboard draws it: the count leads, the list follows. */
  it('lists every warning it was given in one banner, not a sample of them', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ warnings: ['one', 'two', 'three', 'four', 'five'].map((each) => warning(each)) }),
    );
    const { container } = show();

    await screen.findByText('one');
    // The dashboard caps at three as a summary, and this page is the full list.
    expect(container.querySelectorAll('.banner-warning')).toHaveLength(1);
    expect(container.querySelectorAll('.banner-warning li')).toHaveLength(5);
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
        {
          id: 'tmdb',
          display_name: 'TMDB',
          needs_key: true,
          configured: false,
          connected: null,
          reason: null,
        },
        {
          id: 'omdb',
          display_name: 'OMDb',
          needs_key: true,
          configured: true,
          connected: false,
          reason: 'key_refused',
        },
        {
          id: 'arr',
          display_name: 'Radarr / Sonarr',
          needs_key: false,
          configured: true,
          connected: true,
          reason: null,
        },
      ]),
    );
    const { container } = show();

    await screen.findByText('TMDB');
    const rows = [...container.querySelectorAll('.source-row')];
    expect(within(rows[0] as HTMLElement).getByText('needs an API key')).toBeTruthy();
    expect(within(rows[1] as HTMLElement).getByText('Error')).toBeTruthy();
    expect(within(rows[2] as HTMLElement).getByText('Connected')).toBeTruthy();
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
          reason: null,
        },
      ]),
    );
    const { container } = show();

    await screen.findByText('Active');
    // Scoped to the row: the footer's database badge says "Connected" too.
    const row = container.querySelector('.source-row') as HTMLElement;
    expect(within(row).getByText('Active')).toBeTruthy();
    expect(within(row).queryByText('Connected')).toBeNull();
  });

  it('says when each instance last sent a webhook, and which event', async () => {
    const heard = new Date(Date.now() - 120_000).toISOString();
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({
        instances: [
          healthInstance({ last_webhook_at: heard, last_webhook_event: 'MovieAdded' }),
          healthInstance({ id: 'i2', name: 'Radarr 4K' }),
        ],
      }),
    );
    show();

    const heardCell = (await screen.findByText('MovieAdded')).closest('td') as HTMLElement;
    expect(heardCell).toHaveTextContent('2 minutes ago');
    const silent = screen.getByText('Radarr 4K').closest('tr') as HTMLElement;
    expect(within(silent).getAllByText('Never').length).toBeGreaterThan(0);
  });

  it('reports an instance failure in its words, and what the server ran into beside them', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({
        instances: [
          healthInstance({
            status: 'error: Nothing answers at the address',
            detail: 'Nothing answers at the address',
            version: null,
          }),
        ],
      }),
    );
    show();

    const failed = await screen.findByText('Error');
    expect(failed.closest('td')).toHaveTextContent('Nothing answers at the address');
  });

  /**
   * Zero mapped folders is already a warning: painted red, the count would read
   * as a second problem.
   */
  it('reports mapped folders as a count, without a verdict of its own', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ instances: [healthInstance({ mapped_root_folders: 0 })] }),
    );
    const { container } = show();

    await screen.findByText('Radarr');
    // The row's titles are a count too, so the cell is the one reading 0.
    const cell = [...container.querySelectorAll('.num')].find((node) => node.textContent === '0');
    expect(cell).toBeDefined();
    expect(cell?.className).not.toContain('danger');
  });

  it('invites a first instance instead of showing an empty table', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    show();

    expect(await screen.findByText('No instance connected yet.')).toBeTruthy();
  });
});
