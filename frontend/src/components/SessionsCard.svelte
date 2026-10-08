<script lang="ts">
  import { ShieldCheck } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { capitalize, formatTimestamp } from '../api/format';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { handFocus } from '../lib/focus';
  import Loading from './Loading.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  /**
   * The live sessions, which browser holds each, and the way to end them: one
   * left open on a shared machine, or every one at once.
   */
  let { outcome }: { outcome: Outcome } = $props();

  const sessions = createAsync((signal) => api.getSessions(signal));
  let busy = $state(false);

  /**
   * How many sessions show before the rest is asked for. Every browser that
   * signs in opens one, so the list grows with them, and unbounded it would
   * push the settings below it out of reach.
   */
  const SHOWN = 5;
  let everyOne = $state(false);
  // This browser's first: the one most often looked for.
  const ordered = $derived(
    [...(sessions.data ?? [])].sort((a, b) => Number(b.current) - Number(a.current)),
  );
  const shown = $derived(everyOne ? ordered : ordered.slice(0, SHOWN));

  const endId = (index: number) => `session-end-${index}`;

  async function end(handle: string, index: number, current: boolean) {
    const question = t(current ? 'ConfirmEndThisSession' : 'ConfirmEndSession');
    if (!(await askConfirmation(question, 'EndSession'))) return;
    busy = true;
    try {
      await api.endSession(handle);
      // Ending its own session leaves this browser signed out.
      if (current) return location.reload();
      outcome.succeed(t('SessionEnded'));
      await sessions.reload();
    } catch (err) {
      outcome.fail(err);
    } finally {
      busy = false;
      void handFocus(endId(index), endId(index - 1), 'sessions-end-all');
    }
  }

  async function endAll() {
    if (!(await askConfirmation(t('ConfirmEndEverySession'), 'SignOutEverywhere'))) return;
    busy = true;
    try {
      await api.endEverySession();
      location.reload();
    } catch (err) {
      outcome.fail(err);
      busy = false;
    }
  }
</script>

<div class="card">
  <div class="card-header">
    <div>
      <h2 class="card-title flex items-center gap-2">
        <ShieldCheck size={18} aria-hidden="true" />
        {t('Sessions')}
      </h2>
      <p class="card-note">{t('SessionsHelp')}</p>
    </div>
    <button
      id="sessions-end-all"
      type="button"
      class="btn btn-danger btn-sm"
      disabled={busy}
      onclick={() => void endAll()}
    >
      {t('SignOutEverywhere')}
    </button>
  </div>

  {#if sessions.error}
    <ErrorBanner message={sessions.error} onRetry={() => void sessions.reload()} />
  {/if}
  {#if sessions.loading && sessions.data === null}
    <Loading />
  {:else if sessions.data?.length === 0}
    <p class="text-muted">{t('NoSessions')}</p>
  {:else if sessions.data}
    <div class="flex-col gap-2">
      {#each shown as session, index (session.handle)}
        <div class="flex gap-2 items-center">
          <div class="flex-1">
            <!-- A key names nobody: its source says all there is. -->
            {#if session.source !== 'apikey'}
              <span class="text-md">{session.subject}</span>
            {/if}
            <span class="badge badge-kind">{t(`SessionSource${capitalize(session.source)}`)}</span>
            {#if session.current}
              <span class="badge badge-info">{t('ThisSession')}</span>
            {/if}
            <div class="text-muted text-sm">
              {t('SessionTimes', {
                opened: formatTimestamp(session.created_at, i18n.language),
                used: formatTimestamp(session.last_used_at, i18n.language),
                ends: formatTimestamp(session.expires_at, i18n.language),
              })}
            </div>
          </div>
          <button
            id={endId(index)}
            type="button"
            class="btn btn-secondary btn-sm"
            disabled={busy}
            aria-label="{t('EndSession')} – {session.subject}, {formatTimestamp(
              session.created_at,
              i18n.language,
            )}"
            onclick={() => void end(session.handle, index, session.current)}
          >
            {t('EndSession')}
          </button>
        </div>
      {/each}
    </div>
    {#if ordered.length > SHOWN}
      <button
        type="button"
        class="btn btn-secondary btn-sm mt-3"
        onclick={() => (everyOne = !everyOne)}
      >
        {everyOne
          ? t('SessionsShowFewer')
          : t('SessionsShowMore', { count: ordered.length - SHOWN })}
      </button>
    {/if}
  {/if}
</div>
