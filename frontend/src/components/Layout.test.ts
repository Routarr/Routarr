import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { invalidateStatus } from '../lib/status.svelte';
import { ApiError, api } from '../api/client';
import type { Status } from '../api/types';
import LayoutHarness from '../test/LayoutHarness.svelte';
import { withBase } from '../test/base';

/**
 * The chrome. It answers one question above all others — will the next click
 * write to Radarr — and it must answer it even when the backend does not.
 *
 * It reads `/status`, never `/health`: the latter probes every Arr over the
 * network, which would hold the whole shell behind an unreachable server.
 */

const STRINGS = {
  SignOut: 'Sign out',
  DryRunActive: 'Dry-run: writes blocked',
  LiveModeActive: 'Live: writes enabled',
  ModeDryRunShort: 'Dry-run',
  ModeLiveShort: 'Live',
  ListSeparator: ', ',
  AttentionRequired: 'Needs attention: {detail}',
  StatusUnavailable: 'Status unavailable',
  TasksRunning: '{count} running',
  DecisionsAwaitingReview: '{count} awaiting review',
  FailedMoves: '{count} failed',
  DiagnosticWarnings: '{count} warnings',
  OpenNavigation: 'Open navigation',
  ApiKeyRequired: 'This Routarr needs an API key',
  Dashboard: 'Dashboard',
  MainNavigation: 'Main navigation',
  NavGroupSupervision: 'Monitoring',
  Diagnostics: 'Diagnostics',
};

function status(over: Partial<Status> = {}): Status {
  return {
    version: '0.1.0',
    dry_run: true,
    running_jobs: 0,
    pending_decisions: 0,
    failed_decisions: 0,
    warnings: [],
    ...over,
  };
}

function show() {
  // The theme is a server setting, fetched on mount. Unmocked it reaches for a
  // dev server that is not running and fills the output with connection errors.
  vi.spyOn(api, 'getSettings').mockResolvedValue({ ui_theme: 'dark' });
  return renderWithI18n(LayoutHarness, { strings: STRINGS });
}

afterEach(() => {
  withBase(null);
  vi.restoreAllMocks();
});

describe('Layout', () => {
  it('says the next click will only simulate', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status({ dry_run: true }));
    show();

    // Short on screen, the whole sentence as the accessible name: the chrome
    // must not change width with the language.
    const chip = await screen.findByLabelText('Dry-run: writes blocked');
    expect(chip).toHaveTextContent('Dry-run');
    expect(chip.querySelector('.mode-dot')?.className).toContain('is-held');
  });

  /**
   * A mode is not an alert. `LiveModeActive` is the state this application is
   * meant to run in, and it was painted `badge-danger` permanently — the most
   * urgent colour in the palette spent on "nothing is wrong", which is how a
   * red badge stops meaning anything at all.
   */
  it('states the writing mode without spending the danger colour on the normal one', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status({ dry_run: false }));
    show();

    const chip = await screen.findByLabelText('Live: writes enabled');
    expect(chip).toHaveTextContent('Live');
    expect(chip.className).not.toContain('badge-danger');
    expect(chip.querySelector('.mode-dot')?.className).toContain('is-live');
  });

  /**
   * An unreachable backend must not render as an absent warning. Whether
   * writing is possible is the one thing this bar exists to answer, and silence
   * reads as "nothing to worry about" — the opposite of what is true.
   */
  it('says the state is unknown rather than showing nothing at all', async () => {
    vi.spyOn(api, 'getStatus').mockRejectedValue(new Error('connection refused'));
    show();

    expect(await screen.findByText('Status unavailable')).toBeTruthy();
  });

  /**
   * A key is generated at first start, so a browser without one is the ordinary
   * first visit. Mounting the twelve pages behind the gate instead costs a
   * failed request each and reads as a broken install.
   */
  it('replaces the whole shell when the server asks for a key', async () => {
    vi.spyOn(api, 'getStatus').mockRejectedValue(new ApiError('Unauthorized', 401, 'unauthorized'));
    show();

    expect(await screen.findByText('This Routarr needs an API key')).toBeTruthy();
    expect(screen.queryByText('the page')).toBeNull();
  });

  /** Any other failure is a banner, not a gate: the pages still work. */
  it('does not ask for a key when the failure was something else', async () => {
    vi.spyOn(api, 'getStatus').mockRejectedValue(new ApiError('Boom', 500, 'internal'));
    show();

    await screen.findByText('Status unavailable');
    expect(screen.queryByText('This Routarr needs an API key')).toBeNull();
    expect(screen.getByText('the page')).toBeTruthy();
  });

  /**
   * The counts were four translated sentences in the bar, which measured
   * 878px on a 1280px laptop in French and pushed the document 168px wide at
   * 360px. Each now sits on the destination that answers it, where it can be
   * acted on, and the bar keeps one control for what actually wants a person.
   */
  it('turns each status figure into a count on the navigation', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({ running_jobs: 1, pending_decisions: 12, failed_decisions: 2, warnings: ['a', 'b'] }),
    );
    show();

    await screen.findByText('1 running');
    screen.getByText('12 awaiting review');
    screen.getByText('2 failed');
    screen.getByText('2 warnings');
  });

  it('sends the attention control through the mount point a reverse proxy adds', async () => {
    withBase('/routarr/');
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({ running_jobs: 0, pending_decisions: 0, failed_decisions: 2, warnings: ['a'] }),
    );
    show();

    const entry = (label: string) => screen.getByText(label).closest('a');
    await screen.findByText('2 failed');
    expect(entry('2 failed')?.getAttribute('href')).toBe('/routarr/logs');
    expect(entry('1 warnings')?.getAttribute('href')).toBe('/routarr/health');
  });

  /**
   * `signOut` awaited the request and reloaded; a failure was an unhandled
   * rejection, the button looked dead, and the session stayed open.
   */
  it('says when signing out failed instead of pretending it worked', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    vi.spyOn(api, 'authMode').mockResolvedValue({
      mode: 'forms',
      api_key_configured: false,
      api_key_pinned: false,
    });
    vi.spyOn(api, 'logout').mockRejectedValue(new ApiError('later', 503, 'unavailable'));
    show();

    await fireEvent.click(await screen.findByRole('button', { name: 'Sign out' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('later');
  });

  it('totals what needs acting on into one control, and names both parts', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({ running_jobs: 1, pending_decisions: 12, failed_decisions: 2, warnings: ['a', 'b'] }),
    );
    show();

    // Work in progress is not attention: a running task and a pending decision
    // are counted on their entries and nowhere else.
    const attention = await screen.findByRole('link', {
      name: 'Needs attention: 2 failed, 2 warnings',
    });
    expect(attention).toHaveTextContent('4');
    // A failed move is what the danger colour is for, and failures are listed
    // on the log screen. Landing on Diagnostics, which says nothing about
    // them, is worse than landing nowhere.
    expect(attention.getAttribute('href')).toBe('/logs');
  });

  it('sends a warning-only alert to the screen that lists warnings', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status({ warnings: ['a', 'b'] }));
    show();

    const attention = await screen.findByRole('link', { name: 'Needs attention: 2 warnings' });
    expect(attention.getAttribute('href')).toBe('/health');
  });

  it('shows no count for something there is none of, and no attention at all', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();

    await screen.findByLabelText('Dry-run: writes blocked');
    expect(screen.queryByLabelText(/running/)).toBeNull();
    expect(screen.queryByLabelText(/awaiting review/)).toBeNull();
    expect(screen.queryByLabelText(/failed/)).toBeNull();
    expect(screen.queryByLabelText(/Needs attention/)).toBeNull();
  });

  it('opens and closes the navigation drawer, and says which it is', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    const { container } = show();

    const toggle = await screen.findByRole('button', { name: 'Open navigation' });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');

    await fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');

    // Below the drawer breakpoint the navigation covers the page, so clicking
    // beside it has to dismiss it.
    await fireEvent.click(container.querySelector('.sidebar-scrim') as HTMLElement);
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
  });

  /**
   * The scenario the counters exist for: a screen fixes a warning, and the two
   * places that count them have to agree with it at once.
   *
   * They share one request, so they never disagree with each other — they
   * disagreed with the page, for as long as a minute, because nothing told the
   * shell that a mapping had been made or a key saved.
   */
  it('corrects the bar and the navigation the moment a screen says so', async () => {
    const getStatus = vi
      .spyOn(api, 'getStatus')
      .mockResolvedValueOnce(status({ warnings: ['unmapped', 'no key'] }))
      .mockResolvedValue(status({ warnings: ['no key'] }));
    renderWithI18n(LayoutHarness, { strings: STRINGS });

    // Both read the same answer: the bar's attention control and the entry in
    // the menu.
    expect(await screen.findByLabelText('Needs attention: 2 warnings')).toHaveTextContent('2');
    expect(await screen.findByText('2 warnings')).toBeInTheDocument();

    invalidateStatus();

    expect(await screen.findByLabelText('Needs attention: 1 warnings')).toHaveTextContent('1');
    expect(await screen.findByText('1 warnings')).toBeInTheDocument();
    // Once more, not a burst: the shell owns one request and this re-runs it.
    expect(getStatus).toHaveBeenCalledTimes(2);
  });

  /// A warning that is gone leaves nothing behind, in either place.
  it('removes both counters when the last warning is fixed', async () => {
    vi.spyOn(api, 'getStatus')
      .mockResolvedValueOnce(status({ warnings: ['unmapped'] }))
      .mockResolvedValue(status({ warnings: [] }));
    renderWithI18n(LayoutHarness, { strings: STRINGS });

    expect(await screen.findByText('1 warnings')).toBeInTheDocument();

    invalidateStatus();

    await vi.waitFor(() => expect(screen.queryByLabelText(/Needs attention/)).toBeNull());
    expect(screen.queryByLabelText(/warnings/)).toBeNull();
  });
});
