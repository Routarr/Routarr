<script lang="ts">
  import type { Snippet } from 'svelte';
  import { AlertTriangle, Menu, Search } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import { createAsync, describeError } from '../lib/async.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import { poll } from '../lib/poll.svelte';
  import { applyTheme, t } from '../lib/i18n.svelte';
  import { href, router } from '../lib/router.svelte';
  import { statusRevision } from '../lib/status.svelte';
  import ApiKeyGate from './ApiKeyGate.svelte';
  import LoginGate from './LoginGate.svelte';
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
    () => api.getStatus(),
    () => statusRevision(),
  );

  // Which gate to show, and whether to offer a way out. Public and cheap, and
  // asked once: a browser that has not signed in cannot be asked for a session
  // in order to learn that it needs one.
  const auth = createAsync(() => api.authMode());

  // The theme is a server setting, like the language, so it has to be fetched
  // rather than read from the browser. Following the OS is a choice one makes.
  $effect(() => {
    api
      .getSettings()
      .then((settings) => applyTheme(settings.ui_theme || 'dark'))
      // A theme that cannot be read is not worth an error banner: the default
      // renders perfectly well.
      .catch(() => {});
  });

  // Keep the chrome honest. Polling only while a job runs leaves the dry-run
  // badge — the one thing this bar exists to answer — stale indefinitely when
  // the setting changes anywhere else: another tab, a script, a restore. Idle
  // refreshes are a minute apart, which costs one cheap query and removes the
  // possibility of the bar saying "dry run" while the next click writes — and
  // costs nothing at all while the tab is in the background.
  const busy = $derived((status.data?.running_jobs ?? 0) > 0);
  poll(
    () => void status.reload(),
    () => (busy ? 5000 : 60000),
  );

  // Only reachable below the drawer breakpoint; above it the rail is always in
  // the flow and this stays false.
  let drawer = $state(false);

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
    // this would swallow the keystroke — Firefox's own search bar included —
    // and open nothing at all.
    if (unauthorized) return;
    // Alt and Shift are somebody else's: Ctrl+Shift+K is Firefox's console,
    // and AltGr on Windows sets `ctrlKey` and `altKey` together, so a reader
    // typing a bracketed character would lose it to this.
    if (event.altKey || event.shiftKey) return;
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      palette = true;
    }
  }

  /**
   * The one thing in the bar that is worth interrupting for, or nothing.
   *
   * A failed move and a diagnostic warning are the two counts an operator has
   * to act on; a running task and a pending decision are work in progress and
   * belong on the entry that shows them. The figure is the total and the
   * accessible name enumerates, so the colour is never the only carrier.
   */
  const attention = $derived.by(() => {
    const data = status.data;
    if (!data) return null;
    const failed = data.failed_decisions;
    const warned = data.warnings.length;
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
      // Japanese `、`; `Intl.ListFormat` is the wrong tool here, its narrow
      // unit form joining two clauses with nothing at all in both.
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
       first start, that is the ordinary first visit — so ask for it, rather than
       rendering twelve pages that will each fail their own way. -->
  <ApiKeyGate />
{:else}
  <!-- First in the tab order, so a keyboard user is not walked through twelve
       navigation links on every page before reaching the content. Focus is moved
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
    <!-- Each count on the entry that answers it, rather than four sentences
         in the chrome. The drawer carries them on a phone, where the bar has
         no room for any of them. -->
    <Sidebar
      open={drawer}
      onNavigate={() => (drawer = false)}
      version={status.data?.version}
      counts={{
        jobs: status.data?.running_jobs ?? 0,
        decisions: status.data?.pending_decisions ?? 0,
        failed: status.data?.failed_decisions ?? 0,
        warnings: status.data?.warnings.length ?? 0,
      }}
    />

    <!-- Below the drawer breakpoint the navigation covers the page, so the page
         has to be dismissable by clicking beside it. -->
    {#if drawer}
      <div class="sidebar-scrim" onclick={() => (drawer = false)} aria-hidden="true"></div>
    {/if}

    <main class="main-content" id="main" tabindex="-1">
      <header class="topbar">
        <div class="flex items-center gap-2">
          <button
            class="btn btn-ghost btn-sm sidebar-toggle"
            onclick={() => (drawer = !drawer)}
            aria-label={t('OpenNavigation')}
            aria-expanded={drawer}
            aria-controls="sidebar"
          >
            <Menu size={18} aria-hidden="true" />
          </button>

          <!-- A mode, not an alert. `LiveModeActive` is the state this
               application is meant to run in, and it was painted in the danger
               colour permanently — the palette's most urgent signal spent on
               "nothing is wrong". The dot carries the state, the short word
               names it, and the full sentence is the accessible name, so the
               chrome no longer changes width with the language. -->
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
          <!-- The one thing the bar gained: a field for the question this
               product exists to answer, reachable from every screen. -->
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

          {#if attention}
            <!-- Failures first: they are what `is-critical` is painted for,
                 and they are listed on the log screen, not on diagnostics.
                 Sending an operator to a page that says nothing about the
                 thing that just broke is worse than saying nothing. -->
            <a
              href={href(attention.critical ? '/logs' : '/health')}
              class="attention {attention.critical ? 'is-critical' : ''}"
              aria-label={attention.label}
              title={attention.label}
            >
              <AlertTriangle size={14} aria-hidden="true" />
              {attention.total}
            </a>
          {/if}
          <!-- Only where signing in was possible: a key or a proxy has no
               session to end, and a button that does nothing is worse than
               none. -->
          {#if sessionMode}
            <button class="btn btn-ghost btn-sm" onclick={signOut}>{t('SignOut')}</button>
          {/if}
        </div>
      </header>

      <ErrorBanner message={signOutError} onDismiss={() => (signOutError = null)} />

      <!-- Around the routed page only: a crash in a page must not take the
           sidebar and the status bar with it, since those are what the user
           needs to get somewhere that still works. Keyed on the path, so
           leaving a broken page is enough to clear it. -->
      <div class="page-container">
        {#key router.path}
          <svelte:boundary>
            {@render children()}

            <!-- `unknown`, not inferred: a boundary catches whatever was
                 thrown, and a `throw 'oops'` is as valid as an `Error`. Left
                 unannotated the parameter is an implicit `any` — which the
                 line below happens to handle correctly, and which nothing
                 would have kept correct. -->
            {#snippet failed(error: unknown)}
              <div class="banner banner-danger items-start">
                <AlertTriangle size={16} />
                <div class="flex-1">
                  <strong>{t('UnexpectedError')}</strong>
                  <!-- Shown, not hidden behind "something went wrong": the
                       message is what makes a bug report actionable. -->
                  <div class="mono text-sm mt-2">
                    {error instanceof Error ? error.message : String(error)}
                  </div>
                  <div class="flex gap-2 mt-4">
                    <button class="btn btn-secondary btn-sm" onclick={() => location.reload()}>
                      {t('ReloadPage')}
                    </button>
                  </div>
                </div>
              </div>
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
{/if}
