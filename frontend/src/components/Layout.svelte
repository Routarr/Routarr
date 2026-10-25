<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import { AlertTriangle, HelpCircle, ListChecks, Lock, LogOut, Menu, Search } from '../lib/icons';
  import { ApiError, api, onRefused } from '../api/client';
  import { createAsync, describeError } from '../lib/async.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import PageFailure from './PageFailure.svelte';
  import { poll } from '../lib/poll.svelte';
  import { formatCount } from '../api/format';
  import { i18n, t } from '../lib/i18n.svelte';
  import { href, router } from '../lib/router.svelte';
  import { focusHeadingOf } from '../lib/focus';
  import { screenKey } from '../lib/routes';
  import { invalidateStatus, statusRevision, warningsRevision } from '../lib/status.svelte';
  import { proofRequest, settleProof } from '../lib/proof.svelte';
  import { access, publishAccess } from '../lib/access.svelte';
  import { guideProgress, outsideTheGuide } from '../api/onboarding';
  import {
    onboarding,
    publishOnboarding,
    publishOnboardingFailure,
  } from '../lib/onboarding.svelte';
  import ApiKeyGate from './ApiKeyGate.svelte';
  import LoginGate from './LoginGate.svelte';
  import Sidebar from './Sidebar.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  /**
   * The shell. Its bar reads `/status`, never `/health`, which probes every Arr and would hold
   * the shell behind an unreachable one.
   */
  let { children }: { children: Snippet } = $props();

  // Read again when a screen changed something the warnings count, not at the next idle poll.
  const status = createAsync(
    (signal) => api.getStatus(signal),
    () => [statusRevision(), warningsRevision()],
  );

  // Asked once the gate has let the person in, and at each status read until it
  // answers: what a reader is refused, the guide included, waits on it.
  const me = createAsync(
    (signal) => (status.data && !access.known ? api.getMe(signal) : Promise.resolve(null)),
    () => status.data,
  );
  $effect(() => {
    if (me.data) publishAccess(me.data.read_only);
  });

  // Read again on every navigation and after each write the warnings follow: the screen just
  // left may have done a step. A reader is refused it, so it waits until the shell knows who reads.
  const guide = createAsync(
    (signal) =>
      access.known && !access.readOnly ? api.getOnboarding(signal) : Promise.resolve(null),
    () => [statusRevision(), router.path, access.known],
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

  // Polled while idle too, a minute apart, so the dry-run badge follows a change made in another
  // tab, a script or a restore. `poll` stops in a hidden tab.
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

  // The page under the drawer stays inert until the redraw, and an inert
  // toggle takes no focus.
  async function closeDrawer() {
    drawer = false;
    await tick();
    drawerToggle?.focus();
  }

  let palette = $state(false);
  let about = $state(false);
  /** The help is open, on the screen on display or on one the quick search found. */
  let help = $state<{ screen?: string } | null>(null);

  /** Where a typed `?` is a character rather than a request for help. */
  function typing(target: EventTarget | null): boolean {
    return (
      target instanceof HTMLElement &&
      (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
    );
  }

  /**
   * The shortcut, and the label that teaches it. Firefox binds Ctrl+K to its search bar, so the
   * combination is `preventDefault`ed.
   */
  const mac =
    typeof navigator !== 'undefined' &&
    /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
  const shortcut = $derived(mac ? '⌘K' : 'Ctrl K');

  function onShortcut(event: KeyboardEvent) {
    // The palette lives in the authenticated branch: on the sign-in and key screens this would
    // swallow the keystroke and open nothing.
    if (unauthorized) return;
    // A drawer that covers the page is left the way a dialog is, the focus
    // going back to the button that opened it. An open dialog answers Escape
    // itself.
    if (event.key === 'Escape' && drawer && !document.querySelector('dialog[open]')) {
      void closeDrawer();
      return;
    }
    // `?` opens the help of the screen on display. A layout types it with Shift,
    // so it is read before the modifiers are turned away. Over an open dialog
    // the dialog holds the reader, and in a field it is a character.
    if (
      event.key === '?' &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !typing(event.target) &&
      !document.querySelector('dialog[open]')
    ) {
      event.preventDefault();
      help = {};
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
   * The failed moves and diagnostic warnings, the counts an operator acts on: running tasks and
   * pending decisions sit on their own entries. The figure is the total, the name enumerates.
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
      // Named for what it is, since the navigation carries the same figures. Joined with
      // `ListSeparator`, which `Intl.ListFormat`'s narrow unit form leaves out in two languages.
      label: t('AttentionRequired', { detail: parts.join(t('ListSeparator')) }),
    };
  });

  const unauthorized = $derived(
    status.failure instanceof ApiError && status.failure.status === 401,
  );
  // A refusal any screen meets reads the status again, so the gate replaces the shell at once.
  // Not once the shell knows, nor while its own read is out, whose refusal is the answer.
  $effect(() =>
    onRefused(() => {
      if (!unauthorized && !status.loading) invalidateStatus();
    }),
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

  /** What the shell says went wrong, and whether a reload is what mends it. */
  let shellError = $state<{ message: string; reload: boolean } | null>(null);

  /**
   * A dialog's code, fetched when it first opens. A tab left open across an update asks for a
   * chunk the server no longer holds: the dialog closes and the shell offers the reload.
   */
  async function loaded<T>(module: Promise<T>, close: () => void): Promise<T> {
    try {
      return await module;
    } catch {
      close();
      shellError = { message: t('DialogLoadFailed'), reload: true };
      // Closed, the block awaiting it is gone, and nothing is left to settle.
      return new Promise<never>(() => {});
    }
  }

  async function signOut() {
    try {
      await api.logout();
    } catch (err) {
      // A session the server does not know is signed out already. Anything else has to be said,
      // or the button reads as dead and the session stays open on a shared machine.
      if (!(err instanceof ApiError && err.status === 401)) {
        shellError = { message: describeError(err), reload: false };
        return;
      }
    }
    location.reload();
  }
</script>

<!-- On the window, so the shortcut works wherever the focus is. -->
<svelte:window onkeydown={onShortcut} />

{#if unauthorized && sessionMode}
  <LoginGate mode={sessionMode} />
{:else if unauthorized}
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
      onClose={() => void closeDrawer()}
      version={status.data?.version}
      onAbout={() => (about = true)}
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

    <!-- The bar beside the page rather than in it: a `header` inside `main` is no banner, and
         the skip link would land before the bar's tools. -->
    <div class="main-content">
      <header class="topbar" inert={narrow && drawer}>
        <div class="flex items-center gap-2">
          <button
            bind:this={drawerToggle}
            class="btn btn-ghost btn-sm sidebar-toggle topbar-icon"
            onclick={() => void toggleDrawer()}
            aria-label={t('OpenNavigation')}
            aria-expanded={drawer}
            aria-controls="sidebar"
          >
            <Menu size={18} aria-hidden="true" />
          </button>

          {#if status.error}
            <!-- An unreachable backend says so: nothing would read as no warning, and the last
                 mode read may be stale. -->
            <span class="badge badge-danger" role="status" title={status.error}
              >{t('StatusUnavailable')}</span
            >
          {:else if status.data}
            {@const held = status.data.dry_run}
            <!-- A mode, not an alert: live is the state Routarr is meant to run in, so it takes
                 no danger colour. The dot and the short word show it, and the full sentence is
                 the accessible name, on a `role="status"` since ARIA refuses `aria-label` on a
                 bare `<span>`. -->
            <span
              class="mode-chip"
              role="status"
              title={t(held ? 'DryRunActive' : 'LiveModeActive')}
              aria-label={t(held ? 'DryRunActive' : 'LiveModeActive')}
            >
              <i class="mode-dot {held ? 'is-held' : 'is-live'}" aria-hidden="true"></i>
              {t(held ? 'ModeDryRunShort' : 'ModeLiveShort')}
            </span>
          {/if}
          {#if access.readOnly}
            <!-- Beside the mode, as the other thing that says what a click can do.
                 The word goes on a phone, as the bar's other labels do, and the
                 name keeps it. -->
            <span
              class="mode-chip"
              role="status"
              aria-label={t('ReadOnly')}
              title={t('ReadOnlyHelp')}
            >
              <Lock size={12} aria-hidden="true" />
              <span class="topbar-label">{t('ReadOnly')}</span>
            </span>
          {/if}
          <!-- What the state calls for beside the state itself: these come and
               go, and on this side they never move the tools on the other. -->
          {#if attention}
            <!-- Failures first, which the move log lists and Diagnostics does not. -->
            <a
              href={href(attention.critical ? '/move-log' : '/diagnostics')}
              class="attention {attention.critical ? 'is-critical' : ''}"
              title={attention.label}
            >
              <AlertTriangle size={14} aria-hidden="true" />
              {formatCount(attention.total, i18n.language)}
              <span class="visually-hidden">{attention.label}</span>
            </a>
          {/if}
          {#if guidePill}
            <!-- A figure and a glyph, the same width in every language, like
                 the chips beside it. The sentence is the accessible name. -->
            <a
              href={href('/')}
              class="guide-pill"
              title={t('GuidePillLabel', { done: guidePill.done, total: guidePill.total })}
            >
              <ListChecks size={14} aria-hidden="true" />
              {formatCount(guidePill.done, i18n.language)}/{formatCount(
                guidePill.total,
                i18n.language,
              )}
              <!-- After the figure, so the name starts with what the eye reads:
                   a speech input user says what they see. -->
              <span class="visually-hidden">
                {t('GuidePillLabel', { done: guidePill.done, total: guidePill.total })}
              </span>
            </a>
          {/if}
        </div>

        <div class="flex items-center gap-2">
          <button
            type="button"
            class="palette-trigger"
            onclick={() => (palette = true)}
            aria-label={t('CommandPalette')}
            aria-keyshortcuts={mac ? 'Meta+K' : 'Control+K'}
          >
            <Search size={14} aria-hidden="true" />
            <span class="palette-trigger-label">{t('CommandPalette')}</span>
            <kbd class="palette-trigger-key">{shortcut}</kbd>
          </button>

          <button
            type="button"
            class="btn btn-ghost btn-sm topbar-icon"
            onclick={() => (help = {})}
            aria-label={t('Help')}
            aria-keyshortcuts="?"
            title={t('Help')}
          >
            <HelpCircle size={14} aria-hidden="true" />
            <span class="topbar-label">{t('Help')}</span>
          </button>

          <!-- Only where a browser holds a session, the key's included: a proxy or an open
               instance has none to end. -->
          {#if sessionMode || auth.data?.mode === 'apikey'}
            <button
              type="button"
              class="btn btn-ghost btn-sm topbar-icon"
              onclick={signOut}
              aria-label={t('SignOut')}
              title={t('SignOut')}
            >
              <LogOut size={14} aria-hidden="true" />
              <span class="topbar-label">{t('SignOut')}</span>
            </button>
          {/if}
        </div>
      </header>

      <main class="page-main" id="main" tabindex="-1" inert={narrow && drawer}>
        <!-- The badge's reason, which a title would show to a mouse alone. -->
        <ErrorBanner message={status.error} />
        <ErrorBanner
          message={shellError?.message ?? null}
          onReload={shellError?.reload ? () => location.reload() : undefined}
          onDismiss={() => (shellError = null)}
        />

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
  </div>

  <!-- The dialogs sit outside the routed page and its boundary, so a page that navigates or
       throws does not take them down. -->
  <!-- The palette, the help, the proof and About are fetched when they open: the shell is what
       every visit downloads. -->
  {#if palette}
    {#await loaded(import('./CommandPalette.svelte'), () => (palette = false)) then { default: CommandPalette }}
      <CommandPalette
        onClose={() => (palette = false)}
        onHelp={(screen) => {
          palette = false;
          help = { screen };
        }}
      />
    {/await}
  {/if}

  {#if help}
    {#await loaded(import('./HelpPanel.svelte'), () => (help = null)) then { default: HelpPanel }}
      <HelpPanel path={router.path} screen={help.screen} onClose={() => (help = null)} />
    {/await}
  {/if}

  <ConfirmDialog />
  <!-- A proof that cannot be asked for is declined, so every caller unwinds. -->
  {#if proofRequest.request}
    {#await loaded( import('./ProofDialog.svelte'), () => settleProof(null) ) then { default: ProofDialog }}
      <ProofDialog />
    {/await}
  {/if}
  {#if about && status.data}
    {#await loaded(import('./AboutDialog.svelte'), () => (about = false)) then { default: AboutDialog }}
      <AboutDialog version={status.data.version} onClose={() => (about = false)} />
    {/await}
  {/if}
{/if}
