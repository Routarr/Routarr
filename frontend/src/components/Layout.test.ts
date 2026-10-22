import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { ENGLISH, renderWithI18n } from '../test/render';
import { invalidateStatus, invalidateWarnings } from '../lib/status.svelte';
import { ApiError, api } from '../api/client';
import type { OnboardingStatus, Status } from '../api/types';
import LayoutHarness from '../test/LayoutHarness.svelte';
import { withBase } from '../test/base';
import { SCREENS } from '../lib/routes';
import { onboardingStatus, warning } from '../test/fixtures';
import { onboarding, publishOnboarding, publishOnboardingFailure } from '../lib/onboarding.svelte';
import { navigate } from '../lib/router.svelte';
import { access, publishAccess } from '../lib/access.svelte';

/**
 * The chrome. It answers one question above all others (will the next click
 * write to Radarr?) and it must answer it even when the backend does not.
 *
 * It reads `/status`, never `/health`: the latter probes every Arr over the
 * network, which would hold the whole shell behind an unreachable server.
 */

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

/** Who `/auth/me` says reads: the owner, unless a test signs a reader in. */
function signedIn(readOnly = false) {
  return vi
    .spyOn(api, 'getMe')
    .mockResolvedValue({ subject: 'admin', expires_at: null, read_only: readOnly });
}

function show(guide: OnboardingStatus = onboardingStatus([], { state: 'done' }), readOnly = false) {
  vi.spyOn(api, 'getOnboarding').mockResolvedValue(guide);
  signedIn(readOnly);
  return renderWithI18n(LayoutHarness);
}

afterEach(() => {
  vi.useRealTimers();
  withBase(null);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  publishOnboarding(null);
  publishOnboardingFailure(null);
  window.history.replaceState({}, '', '/');
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
   * meant to run in, and `badge-danger` on it would spend the most urgent
   * colour in the palette on "nothing is wrong", which is how a red badge stops
   * meaning anything at all.
   */
  it('states the writing mode without spending the danger colour on the normal one', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status({ dry_run: false }));
    show();

    const chip = await screen.findByLabelText('Live mode: writes enabled');
    expect(chip).toHaveTextContent('Live');
    expect(chip.className).not.toContain('badge-danger');
    expect(chip.querySelector('.mode-dot')?.className).toContain('is-live');
  });

  /**
   * An unreachable backend must not render as an absent warning. Whether
   * writing is possible is the one thing this bar exists to answer, and silence
   * reads as "nothing to worry about", the opposite of what is true.
   */
  it('says the state is unknown rather than showing nothing at all', async () => {
    vi.spyOn(api, 'getStatus').mockRejectedValue(new Error('connection refused'));
    show();

    expect(await screen.findByText('Status unavailable')).toBeTruthy();
  });

  /** A mode read before the server stopped answering is no longer known. */
  it('says the state is unknown once a later read fails', async () => {
    vi.spyOn(api, 'getStatus')
      .mockResolvedValueOnce(status({ dry_run: false }))
      .mockRejectedValue(new Error('connection refused'));
    show();
    await screen.findByLabelText('Live mode: writes enabled');

    invalidateStatus();

    expect(await screen.findByText('Status unavailable')).toBeTruthy();
    expect(screen.queryByLabelText('Live mode: writes enabled')).toBeNull();
  });

  /**
   * A key is generated at first start, so a browser without one is the ordinary
   * first visit. Mounting the pages behind the gate instead costs a failed
   * request each and reads as a broken install.
   */
  it('replaces the whole shell when the server asks for a key', async () => {
    vi.spyOn(api, 'getStatus').mockRejectedValue(new ApiError('Unauthorized', 401, 'unauthorized'));
    show();

    expect(await screen.findByText('This Routarr needs an API key')).toBeTruthy();
    expect(screen.queryByText('the page')).toBeNull();
  });

  /**
   * The gate holds what is being typed into it. A status poll taking it down
   * and putting it back, a minute apart, would take a half-typed key with it,
   * or a half-typed password in the session modes.
   */
  it.each([
    ['apikey', 'Routarr API key'],
    ['forms', 'Password'],
  ] as const)('keeps what is typed into the %s gate through a status poll', async (mode, label) => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(api, 'authMode').mockResolvedValue({
      mode,
      api_key_configured: true,
      api_key_pinned: false,
    });
    vi.spyOn(api, 'getStatus').mockRejectedValue(new ApiError('Unauthorized', 401, 'unauthorized'));
    show();
    const field = (await screen.findByLabelText(label)) as HTMLInputElement;
    await fireEvent.input(field, { target: { value: 'half-typed' } });

    await vi.advanceTimersByTimeAsync(60_000);

    expect((screen.getByLabelText(label) as HTMLInputElement).value).toBe('half-typed');
  });

  /**
   * A session that ended is found by whichever screen asks next, and the
   * sign-in screen follows at once rather than at the next poll, a minute on.
   */
  it('shows the sign-in screen as soon as any request is refused', async () => {
    vi.spyOn(api, 'authMode').mockResolvedValue({
      mode: 'forms',
      api_key_configured: true,
      api_key_pinned: false,
    });
    vi.spyOn(api, 'getStatus')
      .mockResolvedValueOnce(status())
      .mockRejectedValue(new ApiError('Unauthorized', 401, 'unauthorized'));
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          new Response('{"error":"unauthorized","message":"refused"}', { status: 401 }),
        ),
    );
    show();
    await screen.findByText('the page');

    await api.getRules().catch(() => {});

    expect(await screen.findByLabelText('Password')).toBeTruthy();
    vi.unstubAllGlobals();
  });

  /**
   * The shell's own refused read is no news to it: re-read for it, each read
   * would supersede the last before it could say it was refused, for good.
   */
  it('reads its status once when the browser has no session, not in a loop', async () => {
    const asked: string[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        const path = new URL(url, 'http://routarr.test').pathname;
        asked.push(path);
        return path.endsWith('/auth/mode')
          ? new Response('{"mode":"apikey","api_key_configured":true,"api_key_pinned":false}')
          : new Response('{"error":"unauthorized","message":"refused"}', { status: 401 });
      }),
    );
    show();

    expect(await screen.findByText('This Routarr needs an API key')).toBeTruthy();
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(asked.filter((path) => path.endsWith('/status')).length).toBeLessThanOrEqual(2);
    vi.unstubAllGlobals();
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
   * Each count sits on the destination that answers it, where it can be acted
   * on, and the bar keeps one control for what actually wants a person. A
   * sentence per count would outgrow the bar in a long language and widen the
   * document on a phone.
   */
  it('turns each status figure into a count on the navigation', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({
        running_jobs: 1,
        pending_decisions: 12,
        failed_decisions: 2,
        warnings: [warning('a'), warning('b')],
      }),
    );
    show();

    await screen.findByText('Running tasks: 1');
    screen.getByText('Decisions awaiting review: 12');
    screen.getByText('Failed moves: 2');
    screen.getByText('Warnings: 2');
  });

  it('sends the attention control through the mount point a reverse proxy adds', async () => {
    withBase('/routarr/');
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({
        running_jobs: 0,
        pending_decisions: 0,
        failed_decisions: 2,
        warnings: [warning('a')],
      }),
    );
    show();

    const attention = await screen.findByRole('link', { name: /Needs attention/ });
    expect(attention.getAttribute('href')).toBe('/routarr/move-log');
  });

  /**
   * A sign-out that fails unsaid is an unhandled rejection: the button looks
   * dead, and the session stays open on a shared machine.
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

  /**
   * The key is exchanged for a session, so a browser in `apikey` mode holds one
   * like any other, and leaves it the same way.
   */
  it('signs a browser out of the session its API key opened', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    vi.spyOn(api, 'authMode').mockResolvedValue({
      mode: 'apikey',
      api_key_configured: true,
      api_key_pinned: false,
    });
    const logout = vi.spyOn(api, 'logout').mockResolvedValue(undefined as never);
    const reload = vi.fn();
    vi.stubGlobal('location', { ...window.location, reload });
    show();

    await fireEvent.click(await screen.findByRole('button', { name: 'Sign out' }));

    expect(logout).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1));
  });

  /** Nobody signs in there, so there is nothing to leave. */
  it.each(['none', 'external'] as const)('offers no sign-out in %s mode', async (mode) => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    const known = vi.spyOn(api, 'authMode').mockResolvedValue({
      mode,
      api_key_configured: false,
      api_key_pinned: false,
    });
    show();

    await screen.findByText('the page');
    await waitFor(() => expect(known).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByRole('button', { name: 'Sign out' })).toBeNull();
  });

  it('totals what needs acting on into one control, and names both parts', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({
        running_jobs: 1,
        pending_decisions: 12,
        failed_decisions: 2,
        warnings: [warning('a'), warning('b')],
      }),
    );
    show();

    // Work in progress is not attention: a running task and a pending decision
    // are counted on their entries and nowhere else.
    const attention = await screen.findByRole('link', {
      name: /Needs attention: Failed moves: 2, Warnings: 2$/,
    });
    expect(attention).toHaveTextContent('4');
    // A failed move is what the danger colour is for, and failures are listed
    // on the log screen. Landing on Diagnostics, which says nothing about
    // them, is worse than landing nowhere.
    expect(attention.getAttribute('href')).toBe('/move-log');
  });

  it('sends a warning-only alert to the screen that lists warnings', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({ warnings: [warning('a'), warning('b')] }),
    );
    show();

    const attention = await screen.findByRole('link', { name: /Needs attention: Warnings: 2$/ });
    expect(attention.getAttribute('href')).toBe('/diagnostics');
  });

  it('shows no count for something there is none of, and no attention at all', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();

    await screen.findByLabelText('Dry-run: writes blocked');
    expect(screen.queryByText(/Running tasks: \d+/)).toBeNull();
    expect(screen.queryByText(/Decisions awaiting review: \d+/)).toBeNull();
    expect(screen.queryByText(/Failed moves: \d+/)).toBeNull();
    expect(screen.queryByRole('link', { name: /Needs attention/ })).toBeNull();
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

  /** A drawer that covers the page is left the way a dialog is. */
  it('closes the drawer on Escape and hands the focus back to its button', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();

    const toggle = await screen.findByRole('button', { name: 'Open navigation' });
    await fireEvent.click(toggle);
    await fireEvent.keyDown(window, { key: 'Escape' });

    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(document.activeElement).toBe(toggle);
  });

  /**
   * Below the breakpoint a closed drawer is out of sight, and its links must
   * leave the tab order with it. Above it the rail is the navigation.
   */
  it('takes a closed drawer out of reach below the breakpoint, and only there', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    const narrow = (matches: boolean) =>
      vi.stubGlobal('matchMedia', () => ({
        matches,
        addEventListener: () => {},
        removeEventListener: () => {},
      }));

    narrow(true);
    const { container, unmount } = show();
    await screen.findByRole('button', { name: 'Open navigation' });
    const drawer = () => container.querySelector('#sidebar') as HTMLElement;
    expect(drawer().inert).toBe(true);
    await fireEvent.click(screen.getByRole('button', { name: 'Open navigation' }));
    expect(drawer().inert).toBe(false);
    unmount();

    narrow(false);
    const wide = show();
    await screen.findByRole('button', { name: 'Open navigation' });
    expect((wide.container.querySelector('#sidebar') as HTMLElement).inert).toBe(false);
  });

  /** Below the breakpoint an open drawer covers the page, and is used as a dialog is. */
  it('takes the focus into an open drawer and the page out of reach under it', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    vi.stubGlobal('matchMedia', () => ({
      matches: true,
      addEventListener: () => {},
      removeEventListener: () => {},
    }));
    const { container } = show();
    const main = container.querySelector('main') as HTMLElement;

    await fireEvent.click(await screen.findByRole('button', { name: 'Open navigation' }));

    await waitFor(() => expect(document.activeElement).toBe(container.querySelector('#sidebar a')));
    expect(main.inert).toBe(true);
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(main.inert).toBe(false);
  });

  it('leaves the page reachable beside the rail above the breakpoint', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    const { container } = show();

    await fireEvent.click(await screen.findByRole('button', { name: 'Open navigation' }));

    expect((container.querySelector('main') as HTMLElement).inert).toBe(false);
  });

  /**
   * The scenario the counters exist for: a screen fixes a warning, and the two
   * places that count them have to agree with it at once.
   *
   * They share one request, so they never disagree with each other, only with
   * the page, for as long as a minute, unless the screen tells the shell that a
   * mapping was made or a key saved.
   */
  it('corrects the bar and the navigation the moment a screen says so', async () => {
    const getStatus = vi
      .spyOn(api, 'getStatus')
      .mockResolvedValueOnce(status({ warnings: [warning('unmapped'), warning('no key')] }))
      .mockResolvedValue(status({ warnings: [warning('no key')] }));
    signedIn();
    renderWithI18n(LayoutHarness);

    // Both read the same answer: the bar's attention control and the entry in
    // the menu.
    expect(
      await screen.findByRole('link', { name: /Needs attention: Warnings: 2$/ }),
    ).toHaveTextContent('2');
    expect(await screen.findByText('Warnings: 2')).toBeInTheDocument();

    invalidateStatus();

    expect(
      await screen.findByRole('link', { name: /Needs attention: Warnings: 1$/ }),
    ).toHaveTextContent('1');
    expect(await screen.findByText('Warnings: 1')).toBeInTheDocument();
    // Once more, not a burst: the shell owns one request and this re-runs it.
    expect(getStatus).toHaveBeenCalledTimes(2);
  });

  /** A warning that is gone leaves nothing behind, in either place. */
  it('removes both counters when the last warning is fixed', async () => {
    vi.spyOn(api, 'getStatus')
      .mockResolvedValueOnce(status({ warnings: [warning('unmapped')] }))
      .mockResolvedValue(status({ warnings: [] }));
    signedIn();
    renderWithI18n(LayoutHarness);

    expect(await screen.findByText('Warnings: 1')).toBeInTheDocument();

    invalidateStatus();

    await vi.waitFor(() =>
      expect(screen.queryByRole('link', { name: /Needs attention/ })).toBeNull(),
    );
    expect(screen.queryByText(/Warnings: \d+/)).toBeNull();
  });

  /** A fresh install is a list of steps, not a list of alarms. */
  it('shows the guide progress in the chrome, and leaves it the warnings of its open steps', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({ warnings: [warning('No enabled Arr instance', 'instance')] }),
    );
    show(onboardingStatus());

    const pill = await screen.findByRole('link', {
      name: /Getting started, required steps done: 0 of 4$/,
    });
    expect(pill.textContent).toContain('0/4');
    expect(screen.queryByRole('link', { name: /Needs attention/ })).toBeNull();
    expect(screen.queryByText('Warnings: 1')).toBeNull();
  });

  /** No step brings an Arr back up, so the guide never says it. */
  it('counts a warning no step answers while the guide runs', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({
        warnings: [
          warning('Radarr is unreachable'),
          warning('Categories not mapped: 1', 'categories'),
        ],
      }),
    );
    show(onboardingStatus(['instance']));

    expect(await screen.findByRole('link', { name: /Needs attention: Warnings: 1$/ })).toBeTruthy();
    expect(screen.getByRole('link', { name: /Getting started/ })).toBeTruthy();
  });

  it('still flags a failed move while the guide runs', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status({ failed_decisions: 1 }));
    show(onboardingStatus());

    expect(await screen.findByRole('link', { name: /Needs attention/ })).toBeTruthy();
  });

  /** A speech input user says what they see: the name starts with it. */
  it('names the pill and the attention link with the text they show', async () => {
    // Three in all, two failed and one warning: the figure the link shows is in
    // none of the parts its sentence names.
    vi.spyOn(api, 'getStatus').mockResolvedValue(
      status({ failed_decisions: 2, warnings: [warning('Radarr down')] }),
    );
    show(onboardingStatus(['instance', 'categories']));

    const pill = await screen.findByRole('link', { name: /Getting started/ });
    const attention = await screen.findByRole('link', { name: /Needs attention/ });
    for (const link of [pill, attention]) {
      const shown = (link.textContent ?? '').replace(/\s+/g, ' ').trim().split(' ')[0]!;
      expect(link).toHaveAccessibleName(expect.stringContaining(shown));
    }
  });

  it('takes the pill away once the guide is done', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();

    await screen.findByLabelText('Dry-run: writes blocked');
    expect(screen.queryByRole('link', { name: /Getting started/ })).toBeNull();
  });

  /** A dialog holds a draft, which a destination chosen in the palette unmounts. */
  it('opens the palette on Ctrl+K, but not over an open dialog', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();
    await screen.findByLabelText('Dry-run: writes blocked');
    const palette = () => screen.queryByRole('combobox', { name: 'Quick search' });

    // Fetched when it first opens, so it arrives a moment after the key.
    await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    await waitFor(() => expect(palette()).toBeTruthy());
    await fireEvent(palette()!.closest('dialog')!, new Event('cancel', { cancelable: true }));
    await waitFor(() => expect(palette()).toBeNull());

    const dialog = document.createElement('dialog');
    dialog.setAttribute('open', '');
    document.body.append(dialog);
    try {
      await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
      // Already fetched: opened, it would be drawn by the next turn.
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(palette()).toBeNull();
    } finally {
      dialog.remove();
    }
  });

  /** `?` asks for the help of the screen on display, but types a `?` into a field. */
  it('opens the help on ?, and on its button, but not from a field or over a dialog', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();
    await screen.findByLabelText('Dry-run: writes blocked');
    const help = () => screen.queryByRole('dialog', { name: 'Help: Dashboard' });
    const close = async () => {
      await fireEvent(help()!, new Event('cancel', { cancelable: true }));
      await waitFor(() => expect(help()).toBeNull());
    };

    // Fetched when it first opens, so it arrives a moment after the key.
    await fireEvent.keyDown(window, { key: '?', shiftKey: true });
    await waitFor(() => expect(help()).toBeTruthy());
    await close();

    await fireEvent.click(screen.getByRole('button', { name: 'Help' }));
    await waitFor(() => expect(help()).toBeTruthy());
    expect(screen.getByRole('button', { name: 'Help' })).toHaveAttribute('aria-keyshortcuts', '?');
    await close();

    const field = document.createElement('input');
    document.body.append(field);
    const dialog = document.createElement('dialog');
    try {
      await fireEvent.keyDown(field, { key: '?', shiftKey: true });
      dialog.setAttribute('open', '');
      document.body.append(dialog);
      await fireEvent.keyDown(window, { key: '?', shiftKey: true });
      // Already fetched: opened, it would be drawn by the next turn.
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(help()).toBeNull();
    } finally {
      field.remove();
      dialog.remove();
    }
  });

  /**
   * A step is done on the screen the reader just left: the pill, the
   * dashboard list and the step banners follow from this one read.
   */
  it('reads the guide again after a write and on every navigation', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    vi.spyOn(api, 'getOnboarding')
      .mockResolvedValueOnce(onboardingStatus(['instance']))
      .mockResolvedValueOnce(onboardingStatus(['instance', 'categories']))
      .mockResolvedValue(onboardingStatus(['instance', 'categories', 'rule']));
    signedIn();
    renderWithI18n(LayoutHarness);
    const pill = (done: number) =>
      screen.findByRole('link', {
        name: new RegExp(`Getting started, required steps done: ${done} of 4$`),
      });

    expect(await pill(1)).toBeTruthy();
    invalidateStatus();
    expect(await pill(2)).toBeTruthy();
    navigate('/rules');
    expect(await pill(3)).toBeTruthy();
  });

  /**
   * A screen changes without a page load, so nothing tells a screen reader
   * that it did: the tab and the focus say so, as a load would. The first
   * screen is the page load itself and leaves the focus where the browser put
   * it.
   */
  it('names the screen in the tab, and moves the focus to its heading on a navigation', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();

    await vi.waitFor(() => expect(document.title).toBe('Dashboard · Routarr'));
    expect(document.activeElement).toBe(document.body);

    navigate('/diagnostics');
    await vi.waitFor(() => expect(document.title).toBe('Diagnostics · Routarr'));
    await vi.waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('heading', { level: 1 })),
    );

    navigate('/no-such-screen');
    await vi.waitFor(() => expect(document.title).toBe('Page not found · Routarr'));
  });

  /** The dashboard, where the guide lives, says so rather than showing nothing. */
  it('publishes a guide it could not read', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    vi.spyOn(api, 'getOnboarding').mockRejectedValue(new ApiError('locked', 503, 'unavailable'));
    signedIn();
    renderWithI18n(LayoutHarness);

    await vi.waitFor(() => expect(onboarding.failure).toContain('locked'));
  });

  /** A reader is spared the controls the server would refuse, and told so once, in the bar. */
  it('marks the bar read only for a reader, and says what that means', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show(undefined, true);

    const mark = await screen.findByRole('status', { name: 'Read only' });
    expect(mark).toHaveTextContent('Read only');
    expect(mark).toHaveAccessibleDescription(ENGLISH.ReadOnlyHelp);
    // Beside the mode, which still says whether the owner's writes are held.
    expect(screen.getByLabelText('Dry-run: writes blocked')).toBeInTheDocument();
  });

  it('shows the owner no read-only mark', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();

    await vi.waitFor(() => expect(access.known).toBe(true));
    expect(screen.queryByRole('status', { name: 'Read only' })).toBeNull();
  });

  /** Every read those screens make is refused to a reader: a link to one leads to a failure. */
  it('offers a reader none of the screens the owner alone reads', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show(undefined, true);
    await screen.findByRole('status', { name: 'Read only' });

    const navigation = screen.getByRole('navigation', { name: 'Main navigation' });
    for (const name of ['Settings', 'Metadata sources', 'Applications', 'Security log']) {
      expect(within(navigation).queryByRole('link', { name })).toBeNull();
    }
    for (const name of ['Rules', 'Simulation', 'Diagnostics', 'API reference']) {
      expect(within(navigation).getByRole('link', { name })).toBeInTheDocument();
    }
  });

  it('offers the owner every screen', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();
    await vi.waitFor(() => expect(access.known).toBe(true));

    const navigation = screen.getByRole('navigation', { name: 'Main navigation' });
    expect(within(navigation).getAllByRole('link')).toHaveLength(SCREENS.length);
  });

  /**
   * A probe's findings move the warnings and no step of the guide: the status is read again and
   * the guide is not, so an answer of the guide still on its way is not cancelled.
   */
  it('reads the status again when a probe recorded warnings, and not the guide', async () => {
    const read = vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    show();
    await screen.findByLabelText('Dry-run: writes blocked');
    await waitFor(() => expect(api.getOnboarding).toHaveBeenCalledTimes(1));
    const statuses = read.mock.calls.length;

    invalidateWarnings();

    await waitFor(() => expect(read.mock.calls.length).toBeGreaterThan(statuses));
    expect(api.getOnboarding).toHaveBeenCalledTimes(1);
  });

  /** The guide is refused to a reader, and asked before the answer it would be refused too. */
  it('reads the guide once the owner is known, and never for a reader', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    const guide = vi.spyOn(api, 'getOnboarding').mockResolvedValue(onboardingStatus());
    let answer: (readOnly: boolean) => void = () => {};
    vi.spyOn(api, 'getMe').mockReturnValue(
      new Promise((resolve) => {
        answer = (readOnly) => resolve({ subject: 'admin', expires_at: null, read_only: readOnly });
      }),
    );
    const { unmount } = renderWithI18n(LayoutHarness);

    await screen.findByLabelText('Dry-run: writes blocked');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(guide).not.toHaveBeenCalled();
    answer(true);
    await screen.findByRole('status', { name: 'Read only' });
    navigate('/rules');
    invalidateStatus();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(guide).not.toHaveBeenCalled();
    expect(screen.queryByRole('link', { name: /Getting started/ })).toBeNull();
    unmount();

    // The positive control: the same shell, told of the owner, reads it.
    publishAccess(null);
    signedIn();
    renderWithI18n(LayoutHarness);
    expect(await screen.findByRole('link', { name: /Getting started/ })).toBeTruthy();
    expect(guide).toHaveBeenCalled();
  });

  /** Asked behind the gate, the question is one more refusal for a browser with no session. */
  it('asks who reads only once the gate has let the person in', async () => {
    vi.spyOn(api, 'getStatus').mockRejectedValue(new ApiError('Unauthorized', 401, 'unauthorized'));
    const me = signedIn();
    renderWithI18n(LayoutHarness);

    await screen.findByText('This Routarr needs an API key');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(me).not.toHaveBeenCalled();
  });

  /** Unanswered, the guide would wait for good: the next status read asks again. */
  it('asks again who reads at the next status read when the first answer failed', async () => {
    vi.spyOn(api, 'getStatus').mockResolvedValue(status());
    vi.spyOn(api, 'getOnboarding').mockResolvedValue(onboardingStatus());
    const me = vi
      .spyOn(api, 'getMe')
      .mockRejectedValueOnce(new ApiError('later', 503, 'unavailable'))
      .mockResolvedValue({ subject: 'admin', expires_at: null, read_only: false });
    renderWithI18n(LayoutHarness);
    await vi.waitFor(() => expect(me).toHaveBeenCalledTimes(1));
    expect(access.known).toBe(false);

    invalidateStatus();

    expect(await screen.findByRole('link', { name: /Getting started/ })).toBeTruthy();
    expect(me).toHaveBeenCalledTimes(2);
    // Known, it is not asked again at each poll.
    invalidateStatus();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(me).toHaveBeenCalledTimes(2);
  });
});
