import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { formatCount } from '../api/format';
import { health, healthInstance, onboardingStatus, warning } from '../test/fixtures';
import { publishOnboarding, publishOnboardingFailure } from '../lib/onboarding.svelte';
import { statusRevision } from '../lib/status.svelte';
import { api } from '../api/client';
import { withBase } from '../test/base';
import Dashboard from './Dashboard.svelte';
import { publishAccess } from '../lib/access.svelte';

const show = () => renderWithI18n(Dashboard);

/** Both calls answer the same thing unless a test says otherwise. */
const bothReturn = (value: ReturnType<typeof health>) =>
  vi.spyOn(api, 'getHealth').mockResolvedValue(value);

afterEach(() => {
  withBase(null);
  vi.restoreAllMocks();
  vi.useRealTimers();
  publishOnboarding(null);
  publishOnboardingFailure(null);
});

describe('Dashboard', () => {
  it('offers Retry when the dashboard could not be read', async () => {
    const getHealth = vi
      .spyOn(api, 'getHealth')
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockResolvedValue(health());
    show();

    const banner = await screen.findByRole('alert');
    await userEvent.click(within(banner).getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('No instance connected yet.')).toBeTruthy();
    expect(getHealth).toHaveBeenCalledTimes(4);
  });

  it('sends every link through the mount point a reverse proxy adds', async () => {
    withBase('/routarr/');
    bothReturn(health());
    show();

    await screen.findByText('No instance connected yet.');
    const links = screen.getAllByRole('link').map((a) => a.getAttribute('href'));
    expect(links.length).toBeGreaterThan(0);
    for (const link of links) expect(link).toMatch(/^\/routarr\//);
  });

  /** A refresh probes every Arr, which can take a connect timeout, and says it is running. */
  it('says the refresh is running until it ends', async () => {
    vi.spyOn(api, 'getHealth')
      .mockResolvedValueOnce(health())
      .mockResolvedValueOnce(health())
      .mockReturnValue(new Promise(() => {}));
    show();
    const refresh = await screen.findByRole('button', { name: 'Refresh' });
    await waitFor(() => expect(refresh).not.toHaveAttribute('aria-busy', 'true'));

    await fireEvent.click(refresh);

    expect(refresh).toHaveAttribute('aria-busy', 'true');
  });

  it('reads itself again every minute', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const getHealth = vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    show();
    await screen.findByRole('heading', { name: 'Dashboard', level: 1 });
    await waitFor(() => expect(getHealth).toHaveBeenCalledTimes(2));

    await vi.advanceTimersByTimeAsync(60_000);

    expect(getHealth.mock.calls.length).toBeGreaterThan(2);
  });

  it('leads with the number the user came to see', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ stats: { ...health().stats, pending_decisions: 12 } }),
    );
    const { container } = show();

    await screen.findByRole('heading', { name: 'Dashboard', level: 1 });
    // The headline, not one of the context figures below it.
    expect(container.querySelector('.headline-value')?.textContent).toBe('12');
  });

  it('groups the headline figure the way the language does', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ stats: { ...health().stats, pending_decisions: 12345 } }),
    );
    const { container } = renderWithI18n(Dashboard, { language: 'fr' });

    await screen.findByRole('heading', { name: 'Dashboard', level: 1 });
    expect(container.querySelector('.headline-value')?.textContent).toBe(formatCount(12345, 'fr'));
    expect(formatCount(12345, 'fr')).not.toBe('12345');
  });

  it('marks the failure count and only the failure count', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ stats: { ...health().stats, failed_decisions: 3, total_movies: 900 } }),
    );
    const { container } = show();

    await screen.findByText('Failed moves');
    const alerting = [...container.querySelectorAll('.metric.is-danger')];
    expect(alerting).toHaveLength(1);
    // Read after the length assertion, and still checked: `toHaveLength` tells
    // the reader, not the compiler.
    expect(alerting.at(0)?.textContent).toContain('3');
  });

  it('leaves the row unmarked when nothing has failed', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    const { container } = show();

    await screen.findByText('Failed moves');
    expect(container.querySelectorAll('.metric.is-danger')).toHaveLength(0);
  });

  /** The page is a summary: nine warnings must not push the numbers off the screen. */
  it('shows at most three warnings, each with where it is settled, and links to the rest', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({
        warnings: [
          warning('one', null, 'instance_unreachable'),
          ...['two', 'three', 'four', 'five'].map((each) => warning(each)),
        ],
      }),
    );
    const { container } = show();

    await screen.findByText('one');
    expect(container.querySelectorAll('.banner-warning')).toHaveLength(1);
    expect(container.querySelectorAll('.banner-list li')).toHaveLength(3);
    expect(screen.queryByText('four')).toBeNull();
    expect(screen.getAllByRole('link', { name: 'Diagnostics' })[0]).toBeTruthy();
    const first = screen.getByText('one').closest('li')!;
    expect(within(first).getByRole('link', { name: 'Open Instances' })).toHaveAttribute(
      'href',
      '/instances',
    );
  });

  it('says an instance has never synced rather than leaving the cell blank', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ instances: [healthInstance({ last_sync: null })] }),
    );
    show();

    expect(await screen.findByText('Never')).toBeTruthy();
  });

  describe('the two-phase load', () => {
    it('asks for the cheap answer and the probed one', async () => {
      const getHealth = bothReturn(health());
      show();

      await screen.findByRole('heading', { name: 'Dashboard', level: 1 });
      expect(getHealth).toHaveBeenCalledTimes(2);
      expect(getHealth.mock.calls.map((c) => c[0])).toEqual(
        expect.arrayContaining([{ probe: false }, undefined]),
      );
    });

    it('says the status is being checked rather than claiming one', async () => {
      // Only the unprobed answer has landed.
      vi.spyOn(api, 'getHealth').mockImplementation((options) =>
        options?.probe === false
          ? Promise.resolve(health({ instances: [healthInstance({ status: 'unchecked' })] }))
          : new Promise(() => {}),
      );
      show();

      expect(await screen.findByText('Checking…')).toBeTruthy();
      expect(screen.queryByText('Connected')).toBeNull();
    });

    /** A probe that failed is not one still running: the state is unknown. */
    it('stops saying it checks once the probe has failed', async () => {
      vi.spyOn(api, 'getHealth').mockImplementation((options) =>
        options?.probe === false
          ? Promise.resolve(health({ instances: [healthInstance({ status: 'unchecked' })] }))
          : Promise.reject(new Error('connection refused')),
      );
      show();

      expect(await screen.findByText('Unknown')).toBeTruthy();
      expect(screen.queryByText('Checking…')).toBeNull();
    });

    it('replaces it with the real state once the probe answers', async () => {
      vi.spyOn(api, 'getHealth').mockImplementation((options) =>
        Promise.resolve(
          health({
            instances: [
              healthInstance({ status: options?.probe === false ? 'unchecked' : 'connected' }),
            ],
          }),
        ),
      );
      show();

      expect(await screen.findByText('Connected')).toBeTruthy();
      expect(screen.queryByText('Checking…')).toBeNull();
    });
  });

  it('puts the guide first and leaves it the warnings of its open steps', async () => {
    publishOnboarding(onboardingStatus());
    bothReturn(
      health({
        warnings: [warning('No enabled Arr instance', 'instance'), warning('API unauthenticated')],
      }),
    );
    show();

    expect(await screen.findByRole('heading', { name: 'Getting started' })).toBeTruthy();
    expect(await screen.findByText('API unauthenticated')).toBeTruthy();
    expect(screen.queryByText('No enabled Arr instance')).toBeNull();
  });

  it('says the guide could not be read, and reads it again on request', async () => {
    publishOnboardingFailure('database is locked');
    bothReturn(health());
    show();

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(
      'The getting-started guide could not be read: database is locked',
    );
    const before = statusRevision();
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(statusRevision()).toBe(before + 1);
  });
});

describe('for a reader', () => {
  it('shows the owner the guide, and a reader the dashboard without it', async () => {
    publishOnboarding(onboardingStatus());
    bothReturn(health());
    const { unmount } = show();
    expect(await screen.findByRole('heading', { name: 'Getting started' })).toBeInTheDocument();
    unmount();

    publishAccess(true);
    show();

    expect(await screen.findByText('No instance connected yet.')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Getting started' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Skip the guide' })).toBeNull();
  });

  it('says an unprobed instance is unknown to a reader, and still being checked to the owner', async () => {
    bothReturn(health({ instances: [healthInstance({ status: 'unchecked' })] }));
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
