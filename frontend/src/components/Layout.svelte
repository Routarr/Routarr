<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import { AlertTriangle, ListChecks, LogOut, Menu, Search } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import { createAsync, describeError } from '../lib/async.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import PageFailure from './PageFailure.svelte';
  import { poll } from '../lib/poll.svelte';
  import { applyTheme, t } from '../lib/i18n.svelte';
  import { href, router } from '../lib/router.svelte';
  import { focusHeadingOf } from '../lib/focus';
  import { screenKey } from '../lib/routes';
  import { statusRevision } from '../lib/status.svelte';
  import { guideProgress, outsideTheGuide } from '../api/onboarding';
  import {
    onboarding,
    publishOnboarding,
    publishOnboardingFailure,
  } from '../lib/onboarding.svelte';
  import ApiKeyGate from './ApiKeyGate.svelte';
  import LoginGate from './LoginGate.svelte';
  import ProofDialog from './ProofDialog.svelte';
  import Sidebar from './Sidebar.svelte';
  import CommandPalette from './CommandPalette.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  /**
   * The top bar reflects the two things that change what a click will do: whether
   * global dry-run is on, and whether anything is running in the background.
   *
   * It reads `/status`, not `/health`: the latter probes every Arr instance, which
   * would block the whole app shell behind an unreachable server.
   */
  let { children }: { children: Snippet } = $props();

  // Re-read when a screen says it changed something the warnings count, so
  // the bar and the navigation correct themselves at once rather than at the
  // next idle poll a minute later.
  const status = createAsync(
    (signal) => api.getStatus(signal),
    () => statusRevision(),
  );

  // Read again on every navigation and after every write the warnings follow:
  // a step is done by the screen the reader just left, and the pill, the
  // dashboard list and the step banners all read this one answer.
  const guide = createAsync(
    (signal) => api.getOnboarding(signal),
    () => [statusRevision(), router.path],
  );
  $effect(() => {
    if (guide.data) publishOnboarding(guide.data);
    publishOnboardingFailure(guide.error);
  });
  const guidePill = $derived.by(() => {
    const status = onboarding.current;
    if (!status || status.state !== 'pending') return null;
    return guideProgress(status);
  });
  // A warning an open step restates is left to the guide, which the pill
  // points at. A failed move is never a step, so it always shows.
  const warned = $derived(outsideTheGuide(status.data?.warnings ?? [], onboarding.current).length);

  // Which gate to show, and whether to offer a way out. Public and cheap, and
  // asked once: a browser that has not signed in cannot be asked for a session
  // in order to learn that it needs one.
  const auth = createAsync((signal) => api.authMode(signal));

  // The theme is a server setting, like the language, so it has to be fetched
  // rather than read from the browser. Following the OS is a choice one makes.
  $effect(() => {
    api
      .getSettings()
      .then(({ ui_theme }) =>
        applyTheme(typeof ui_theme === 'string' && ui_theme ? ui_theme : 'dark'),
      )
      // A theme that cannot be read is not worth an error banner: the default
      // renders perfectly well.
      .catch(() => {});
  });

  // Keep the chrome honest. Polling only while a job runs would leave the
  // dry-run badge, the one thing this bar exists to answer, stale indefinitely
  // when the setting changes anywhere else: another tab, a script, a restore.
  // Idle refreshes are a minute apart, which costs one cheap query, removes the
  // possibility of the bar saying "dry run" while the next click writes, and
  // costs nothing at all while the tab is in the background.
  const busy = $derived((status.data?.running_jobs ?? 0) > 0);
  poll(
    () => void status.reload(),
    () => (busy ? 5000 : 60000),
  );

  // Only reachable below the drawer breakpoint. Above it the rail is always in
  // the flow and this stays false.
  let drawer = $state(false);
  let drawerToggle = $state<HTMLButtonElement | null>(null);

  // Below the drawer breakpoint of `index.css` a closed drawer is out of sight,
  // and its links leave the tab order with it. Above it the rail is the
  // navigation and stays reachable.
  let narrow = $state(false);
  $effect(() => {
    const query = window.matchMedia?.('(max-width: 900px)');
    if (!query) return;
    narrow = query.matches;
    const follow = () => (narrow = query.matches);
    query.addEventListener('change', follow);
    return () => query.removeEventListener('change', follow);
  });

  // An open drawer covers the page below the breakpoint, so it takes the focus
  // and the page under the scrim leaves the tab order, as under a dialog. The
  // toggle follows the drawer in the document, and left there the focus walks
  // controls nobody can see before it reaches a link.
  async function toggleDrawer() {
    drawer = !drawer;
    if (!drawer || !narrow) return;
    await tick();
    document.querySelector<HTMLElement>('#sidebar a')?.focus();
  }

  let palette = $state(false);

  /**
   * The shortcut, and the label that teaches it.
   *
   * `preventDefault` matters on the combination rather than only for tidiness:
   * Firefox binds Ctrl+K to its own search bar, and without it the browser
   * takes the keystroke and the palette never opens.
   */
  const mac =
    typeof navigator !== 'undefined' &&
    /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
  const shortcut = $derived(mac ? '⌘K' : 'Ctrl K');

  function onShortcut(event: KeyboardEvent) {
    // Guarded on the shell, not only on the combination: the palette is
    // rendered in the authenticated branch, so on the sign-in and key screens
    // this would swallow the keystroke (Firefox's own search bar included) and
    // open nothing at all.
    if (unauthorized) return;
    // A drawer that covers the page is left the way a dialog is, the focus
    // going back to the button that opened it. An open dialog answers Escape
    // itself.
    if (event.key === 'Escape' && drawer && !document.querySelector('dialog[open]')) {
      drawer = false;
      drawerToggle?.focus();
      return;
    }
    // Alt and Shift are somebody else's: Ctrl+Shift+K is Firefox's console,
    // and AltGr on Windows sets `ctrlKey` and `altKey` together, so a reader
    // typing a bracketed character would lose it to this.
    if (event.altKey || event.shiftKey) return;
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      // An open dialog holds the reader's work, a rule half written or an
      // instance form, and a destination chosen in the palette unmounts the
      // page under it, draft and all. The key stays claimed, so the browser's
      // own search does not open in its place.
      if (document.querySelector('dialog[open]')) return;
      palette = true;
    }
  }

  /**
   * The one thing in the bar that is worth interrupting for, or nothing.
   *
   * A failed move and a diagnostic warning are the two counts an operator has
   * to act on. A running task and a pending decision are work in progress and
   * belong on the entry that shows them. The figure is the total and the
   * accessible name enumerates, so the colour is never the only carrier.
   */
  const attention = $derived.by(() => {
    const data = status.data;
    if (!data) return null;
    const failed = data.failed_decisions;
    if (failed === 0 && warned === 0) return null;
    const parts: string[] = [];
    if (failed > 0) parts.push(t('FailedMoves', { count: failed }));
    if (warned > 0) parts.push(t('DiagnosticWarnings', { count: warned }));
    return {
      total: failed + warned,
      critical: failed > 0,
      // Named for what it is, not only for what it counts: the navigation
      // carries the same figures on their own entries, and two controls
      // answering to one accessible name is a control nobody can address.
      // The separator is a dictionary entry because Arabic writes `،` and
      // Japanese `、`. `Intl.ListFormat` is the wrong tool here: its narrow
      // unit form joins two clauses with nothing at all in both.
      label: t('AttentionRequired', { detail: parts.join(t('ListSeparator')) }),
    };
  });

  const unauthorized = $derived(
    status.failure instanceof ApiError && status.failure.status === 401,
  );
  // Until the mode is known, the key gate is the safer guess: it is the default
  // mode, and it tells the user where to find a credential either way.
  const sessionMode = $derived(
    auth.data?.mode === 'forms' || auth.data?.mode === 'oidc' ? auth.data.mode : null,
  );

  // A screen changes without a page load, so the tab, the history list and a
  // screen reader learn of it here. The first screen is the page load itself,
  // which the browser announces and focuses on its own.
  $effect(() => {
    document.title = unauthorized ? 'Routarr' : `${t(screenKey(router.path))} · Routarr`;
  });
  let pageContainer = $state<HTMLElement | null>(null);
  let shownPath: string | null = null;
  $effect(() => {
    const path = router.path;
    if (!pageContainer) return;
    const first = shownPath === null;
    shownPath = path;
    if (!first) return focusHeadingOf(pageContainer);
  });

  let signOutError = $state<string | null>(null);

  async function signOut() {
    try {
      await api.logout();
    } catch (err) {
      // A session the server no longer knows is signed out already. Anything
      // else has to be said, or the button reads as dead and the session is
      // still open on a shared machine.
      if (!(err instanceof ApiError && err.status === 401)) {
        signOutError = describeError(err);
        return;
      }
    }
    location.reload();
  }
</script>

<!-- On the window rather than on an element: the shortcut has to work wherever
     the focus happens to be, and a meta tag cannot sit inside a block. -->
<svelte:window onkeydown={onShortcut} />

{#if unauthorized && sessionMode}
  <LoginGate mode={sessionMode} />
{:else if unauthorized}
  <!-- The server wants a key this browser has not got. Since one is generated at
       first start, that is the ordinary first visit, so ask for it rather than
       render every page failing its own way. -->
  <ApiKeyGate />
{:else}
  <!-- First in the tab order, so a keyboard user is not walked through every
       navigation link on every page before reaching the content. Focus is moved
       by hand: the backend injects a `<base href>`, against which a bare `#main`
       resolves to the mount point and reloads the page instead of jumping. -->
  <a
    class="skip-link"
    href="#main"
    onclick={(event) => {
      event.preventDefault();
      document.getElementById('main')?.focus();
    }}>{t('SkipToContent')}</a
  >
  <div class="app-container">
    <!-- Each count on the entry that answers it, rather than as a sentence
         in the chrome. The drawer carries them on a phone, where the bar has
         no room for any of them. -->
    <Sidebar
      open={drawer}
      offstage={narrow && !drawer}
      onNavigate={() => (drawer = false)}
      version={status.data?.version}
      counts={{
        jobs: status.data?.running_jobs ?? 0,
        decisions: status.data?.pending_decisions ?? 0,
        failed: status.data?.failed_decisions ?? 0,
        warnings: warned,
      }}
    />

    <!-- Below the drawer breakpoint the navigation covers the page, so the page
         has to be dismissable by clicking beside it. -->
    {#if drawer}
      <div class="sidebar-scrim" onclick={() => (drawer = false)} aria-hidden="true"></div>
    {/if}

    <main class="main-content" id="main" tabindex="-1" inert={narrow && drawer}>
      <header class="topbar">
        <div class="flex items-center gap-2">
          <button
            bind:this={drawerToggle}
            class="btn btn-ghost btn-sm sidebar-toggle"
            onclick={() => void toggleDrawer()}
            aria-label={t('OpenNavigation')}
            aria-expanded={drawer}
            aria-controls="sidebar"
          >
            <Menu size={18} aria-hidden="true" />
          </button>

          <!-- A mode, not an alert. `LiveModeActive` is the state this
               application is meant to run in, and the danger colour on it
               would spend the palette's most urgent signal on "nothing is
               wrong". The dot carries the state, the short word names it, and
               the full sentence is the accessible name, so the chrome keeps
               its width in every language. -->
          {#if status.data}
            {@const held = status.data.dry_run}
            <!-- `role="status"`, as the unreachable-backend badge beside it
                 already carries: a `<span>` with no role maps to `generic`,
                 for which ARIA prohibits `aria-label` and where the whole
                 sentence would reach nobody. -->
            <span
              class="mode-chip"
              role="status"
              title={t(held ? 'DryRunActive' : 'LiveModeActive')}
              aria-label={t(held ? 'DryRunActive' : 'LiveModeActive')}
            >
              <i class="mode-dot {held ? 'is-held' : 'is-live'}" aria-hidden="true"></i>
              {t(held ? 'ModeDryRunShort' : 'ModeLiveShort')}
            </span>
          {:else if status.error}
            <!-- Whether writing is possible is the one thing this bar must
                 always answer. Rendering nothing reads as "no warning", which
                 is the opposite of what an unreachable backend means. -->
            <span class="badge badge-danger" role="status" title={status.error}
              >{t('StatusUnavailable')}</span
            >
          {/if}
        </div>

        <div class="flex items-center gap-2">
          <!-- A field for the question this product exists to answer,
               reachable from every screen. -->
          <button
            type="button"
            class="palette-trigger"
            onclick={() => (palette = true)}
            aria-label={t('CommandPalette')}
          >
            <Search size={14} aria-hidden="true" />
            <span class="palette-trigger-label">{t('CommandPalette')}</span>
            <kbd class="palette-trigger-key">{shortcut}</kbd>
          </button>

          {#if guidePill}
            <!-- A figure and a glyph, the same width in every language, like
                 the chips beside it. The sentence is the accessible name. -->
            <a
              href={href('/')}
              class="guide-pill"
              title={t('GuidePillLabel', { done: guidePill.done, total: guidePill.total })}
            >
              <ListChecks size={14} aria-hidden="true" />
              {guidePill.done}/{guidePill.total}
              <!-- After the figure, so the name starts with what the eye reads:
                   a speech input user says what they see. -->
              <span class="visually-hidden">
                {t('GuidePillLabel', { done: guidePill.done, total: guidePill.total })}
              </span>
            </a>
          {/if}
          {#if attention}
            <!-- Failures first: they are what `is-critical` is painted for,
                 and they are listed on the log screen, not on diagnostics.
                 Sending an operator to a page that says nothing about the
                 thing that just broke is worse than saying nothing. -->
            <a
              href={href(attention.critical ? '/move-log' : '/diagnostics')}
              class="attention {attention.critical ? 'is-critical' : ''}"
              title={attention.label}
            >
              <AlertTriangle size={14} aria-hidden="true" />
              {attention.total}
              <span class="visually-hidden">{attention.label}</span>
            </a>
          {/if}
          <!-- Only where a browser holds a session, the key's included: a
               proxy or an open instance has none to end, and a button that
               does nothing is worse than none. -->
          {#if sessionMode || auth.data?.mode === 'apikey'}
            <button
              type="button"
              class="btn btn-ghost btn-sm"
              onclick={signOut}
              aria-label={t('SignOut')}
              title={t('SignOut')}
            >
              <LogOut size={14} aria-hidden="true" />
              <span class="sign-out-label">{t('SignOut')}</span>
            </button>
          {/if}
        </div>
      </header>

      <ErrorBanner message={signOutError} onDismiss={() => (signOutError = null)} />

      <!-- Around the routed page only: a crash in a page must not take the
           sidebar and the status bar with it, since those are what the user
           needs to get somewhere that still works. Keyed on the path, so
           leaving a broken page is enough to clear it. -->
      <div class="page-container" bind:this={pageContainer}>
        {#key router.path}
          <svelte:boundary>
            {@render children()}

            <!-- `unknown`, not inferred: a boundary catches whatever was
                 thrown, and a `throw 'oops'` is as valid as an `Error`. -->
            {#snippet failed(error: unknown)}
              <PageFailure {error} />
            {/snippet}
          </svelte:boundary>
        {/key}
      </div>
    </main>
  </div>

  <!-- Outside the routed page and outside the boundary, so a question survives
       the navigation it may itself have triggered, and so a page that throws
       does not take the dialog asking about it down with it. -->
  {#if palette}
    <CommandPalette onClose={() => (palette = false)} />
  {/if}

  <ConfirmDialog />
  <ProofDialog />
{/if}
