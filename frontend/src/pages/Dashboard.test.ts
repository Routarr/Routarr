import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { formatCount } from '../api/format';
import { health, healthInstance, onboardingStatus, warning } from '../test/fixtures';
import { publishOnboarding, publishOnboardingFailure } from '../lib/onboarding.svelte';
import { statusRevision } from '../lib/status.svelte';
import { api } from '../api/client';
import { withBase } from '../test/base';
import Dashboard from './Dashboard.svelte';

/**
 * The dashboard answers one question (is there anything waiting for me?), and
 * everything else on it is context. Cards of equal weight, some with a
 * coloured icon tile and some colouring their *number* by sentiment, would be
 * two encoding systems on one row with nothing saying where to look.
 */

const STRINGS = {
  Dashboard: 'Dashboard',
  PendingDecisions: 'Awaiting review',
  MoviesManaged: 'Movies',
  FailedMovesLabel: 'Failed',
  NoInstanceConfigured: 'No instance configured',
  Diagnostics: 'Diagnostics',
  Connected: 'connected',
  Never: 'never',
  Checking: 'checking…',
  MetadataEnrichedCount: '{count} items enriched.',
  MetadataComplete: 'Nothing missing.',
  GuideTitle: 'Getting started',
  GuideUnavailable: 'The getting-started guide could not be read: {error}',
  Retry: 'Retry',
};

const show = () => renderWithI18n(Dashboard, { strings: STRINGS });

/** Both calls answer the same thing unless a test says otherwise. */
const bothReturn = (value: ReturnType<typeof health>) =>
  vi.spyOn(api, 'getHealth').mockResolvedValue(value);

afterEach(() => {
  withBase(null);
  vi.restoreAllMocks();
  publishOnboarding(null);
  publishOnboardingFailure(null);
});

describe('Dashboard', () => {
  /** After a blip the reader asks again from the banner, as on every other screen. */
  it('offers Retry when the dashboard could not be read', async () => {
    const getHealth = vi
      .spyOn(api, 'getHealth')
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockResolvedValue(health());
    show();

    const banner = await screen.findByRole('alert');
    await userEvent.click(within(banner).getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('No instance configured')).toBeTruthy();
    expect(getHealth).toHaveBeenCalledTimes(4);
  });

  it('sends every link through the mount point a reverse proxy adds', async () => {
    withBase('/routarr/');
    bothReturn(health());
    show();

    await screen.findByText('No instance configured');
    const links = screen.getAllByRole('link').map((a) => a.getAttribute('href'));
    expect(links.length).toBeGreaterThan(0);
    for (const link of links) expect(link).toMatch(/^\/routarr\//);
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

  /** Grouped as the language groups digits, as every figure beside it is. */
  it('groups the headline figure the way the language does', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ stats: { ...health().stats, pending_decisions: 12345 } }),
    );
    const { container } = renderWithI18n(Dashboard, { strings: STRINGS, language: 'fr' });

    await screen.findByRole('heading', { name: 'Dashboard', level: 1 });
    expect(container.querySelector('.headline-value')?.textContent).toBe(formatCount(12345, 'fr'));
    expect(formatCount(12345, 'fr')).not.toBe('12345');
  });

  /**
   * One number on the row changes colour, because a failure is the only one of
   * these that asks the user for something. If everything could go red the
   * colour would say nothing.
   */
  it('marks the failure count and only the failure count', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ stats: { ...health().stats, failed_decisions: 3, total_movies: 900 } }),
    );
    const { container } = show();

    await screen.findByText('Failed');
    const alerting = [...container.querySelectorAll('.metric.is-danger')];
    expect(alerting).toHaveLength(1);
    // Read after the length assertion, and still checked: `toHaveLength` tells
    // the reader, not the compiler.
    expect(alerting.at(0)?.textContent).toContain('3');
  });

  it('leaves the row unmarked when nothing has failed', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(health());
    const { container } = show();

    await screen.findByText('Failed');
    expect(container.querySelectorAll('.metric.is-danger')).toHaveLength(0);
  });

  /**
   * Warnings are capped at three. The page is a summary: a diagnostics run that
   * finds nine problems must not push the numbers off the screen, and the link
   * to the full list is what makes the cap honest.
   */
  it('shows at most three warnings, and links to the rest', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ warnings: ['one', 'two', 'three', 'four', 'five'].map((each) => warning(each)) }),
    );
    const { container } = show();

    await screen.findByText('one');
    // One block, not one banner each: stacked bars would say the same thing
    // once per warning, each with its own copy of the same button.
    expect(container.querySelectorAll('.banner-warning')).toHaveLength(1);
    expect(container.querySelectorAll('.banner-list li')).toHaveLength(3);
    expect(screen.queryByText('four')).toBeNull();
    expect(screen.getAllByRole('link', { name: 'Diagnostics' })[0]).toBeTruthy();
  });

  it('says an instance has never synced rather than leaving the cell blank', async () => {
    vi.spyOn(api, 'getHealth').mockResolvedValue(
      health({ instances: [healthInstance({ last_sync: null })] }),
    );
    show();

    expect(await screen.findByText('never')).toBeTruthy();
  });

  /**
   * Two requests: one that answers from the database in milliseconds, one that
   * probes every Arr over the network. Probing an unreachable Arr costs the
   * full connect timeout, up to five seconds, on the screen somebody opens
   * *because* an Arr is unreachable. The page renders on the first and upgrades
   * on the second.
   */
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

      expect(await screen.findByText('checking…')).toBeTruthy();
      expect(screen.queryByText('connected')).toBeNull();
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

      expect(await screen.findByText('connected')).toBeTruthy();
      expect(screen.queryByText('checking…')).toBeNull();
    });
  });

  /** A warning that restates an open step is the step's, and the guide says it. */
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

  /** Where the guide lives, a guide that could not be read is said, with a way to try again. */
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
