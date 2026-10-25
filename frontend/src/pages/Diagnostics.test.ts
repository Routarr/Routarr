import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { health, healthInstance, onboardingStatus, warning } from '../test/fixtures';
import { api } from '../api/client';
import type { Health } from '../api/types';
import { statusRevision, warningsRevision } from '../lib/status.svelte';
import { publishAccess } from '../lib/access.svelte';
import { publishOnboarding } from '../lib/onboarding.svelte';
import Diagnostics from './Diagnostics.svelte';

const show = () => renderWithI18n(Diagnostics);

const withProviders = (list: Health['metadata']['providers']) =>
  health({ metadata: { providers: list, cached_items: 0, media_missing_metadata: 0 } });

afterEach(() => {
  vi.restoreAllMocks();
  publishOnboarding(null);
});

describe('Diagnostics', () => {
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

  it('has the shell count again what a probe found, and leaves the guide alone', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    const [warnings, guide] = [warningsRevision(), statusRevision()];
    show();

    await screen.findByText('Everything checks out.');
    expect(warningsRevision()).toBeGreaterThan(warnings);
    expect(statusRevision()).toBe(guide);
  });

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

  it('counts the warnings the guide restates apart, as the bar does, and names their step', async () => {
    publishOnboarding(onboardingStatus());
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({
        warnings: [
          warning('Rule tests failing: 1 of 1.', null, 'rule_tests_failing'),
          warning('Categories not mapped: 2.', 'categories', 'unmapped_categories'),
          warning("'Radarr 4K' has no folder mapped.", 'categories', 'instance_without_mapping'),
        ],
      }),
    );
    show();

    expect(
      await screen.findByText('Warnings: 1, and 2 in the getting-started guide'),
    ).toBeInTheDocument();
    const [failing, unmapped] = screen.getAllByRole('listitem');
    expect(failing).not.toHaveTextContent('getting-started guide');
    expect(unmapped).toHaveTextContent(
      'Also in the getting-started guide: Map root folders to categories',
    );

    // Once the guide is done, every warning counts.
    publishOnboarding(onboardingStatus([], { state: 'done' }));
    expect(await screen.findByText('Warnings: 3')).toBeInTheDocument();
    expect(screen.queryByText(/getting-started guide/)).toBeNull();
  });

  it('links each warning to the screen that settles it, and offers nothing where none does', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({
        warnings: [
          warning('Rule tests failing: 1 of 1.', null, 'rule_tests_failing'),
          warning('The disk is nearly full.', null, 'data_disk_low'),
        ],
      }),
    );
    show();

    const failing = (await screen.findByText('Rule tests failing: 1 of 1.')).closest('li')!;
    expect(within(failing).getByRole('link', { name: 'Open Rule tests' })).toHaveAttribute(
      'href',
      '/rule-tests',
    );
    const disk = screen.getByText('The disk is nearly full.').closest('li')!;
    expect(within(disk).queryByRole('link')).toBeNull();
  });

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

  /** A source its pace or its day's quota holds back answers again on its own: no error. */
  it('reads a source held back by its pace or its quota apart from a broken one', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      withProviders([
        {
          id: 'tenrai',
          display_name: 'Tenrai (MyAnimeList)',
          needs_key: false,
          configured: true,
          connected: false,
          reason: 'rate_limited',
        },
        {
          id: 'omdb',
          display_name: 'OMDb',
          needs_key: true,
          configured: true,
          connected: false,
          reason: 'quota_spent',
        },
      ]),
    );
    const { container } = show();

    await screen.findByText('OMDb');
    for (const row of container.querySelectorAll('.source-row')) {
      const badge = within(row as HTMLElement).getByText('Held back');
      expect(badge.classList.contains('badge-warning')).toBe(true);
    }
    expect(screen.queryByText('Error')).toBeNull();
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
          healthInstance({
            last_webhook_at: heard,
            last_webhook_event: 'MovieAdded',
            last_webhook_event_label: 'On Movie Added',
          }),
          healthInstance({ id: 'i2', name: 'Radarr 4K' }),
        ],
      }),
    );
    show();

    const label = await screen.findByText('On Movie Added');
    expect(label.classList.contains('mono')).toBe(true);
    expect(label.closest('td')).toHaveTextContent('2 minutes ago');
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

describe('for a reader', () => {
  it('says an unprobed instance is unknown to a reader, and still being checked to the owner', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ instances: [healthInstance({ status: 'unchecked' })] }),
    );
    const { unmount } = show();
    expect(await screen.findByText('Checking…')).toBeInTheDocument();
    unmount();

    publishAccess(true);
    show();

    const row = await screen.findByRole('row', { name: /Radarr/ });
    expect(within(row).getByText('Unknown')).toBeInTheDocument();
    expect(screen.queryByText('Checking…')).toBeNull();
  });
});
