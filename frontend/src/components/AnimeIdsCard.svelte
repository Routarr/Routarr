<script lang="ts">
  import type { Snippet } from 'svelte';
  import CardTitle from './CardTitle.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import FileButton from './FileButton.svelte';
  import OutcomeBanner from './OutcomeBanner.svelte';
  import { api } from '../api/client';
  import type { AnimeIdsLoaded } from '../api/types';
  import { formatCount, formatRelative } from '../api/format';
  import { access } from '../lib/access.svelte';
  import { handFocus } from '../lib/focus';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { Download } from '../lib/icons';
  import { createOutcome } from '../lib/outcome.svelte';

  /**
   * The anime identifier table: whether one is loaded, and the two ways to load one. Routarr
   * never fetches it unless someone presses Download or turns on the weekly download.
   */
  let {
    setting,
  }: {
    /** The weekly download's field, saved by the screen's bar. */
    setting?: Snippet;
  } = $props();

  const table = createAsync((signal) => api.getAnimeIds(signal));
  const outcome = createOutcome();
  let loading = $state(false);

  /** Stops following, not the load, which runs on whatever the card does. */
  const leaving = new AbortController();
  $effect(() => () => leaving.abort());
  const following = { signal: leaving.signal };

  async function load(how: () => Promise<AnimeIdsLoaded>) {
    const pressed = document.activeElement as HTMLElement | null;
    loading = true;
    try {
      const loaded = await how();
      outcome.succeed(
        t('AnimeIdsLoaded', {
          count: formatCount(loaded.entries, i18n.language),
          changed: formatCount(loaded.titles, i18n.language),
        }),
      );
    } catch (err) {
      if (!leaving.signal.aborted) outcome.fail(err);
    } finally {
      loading = false;
      if (!leaving.signal.aborted) {
        await table.reload();
        void handFocus(pressed);
      }
    }
  }
</script>

<div class="card-header">
  <CardTitle card="AnimeIds">{t('AnimeIds')}</CardTitle>
</div>
<p class="text-muted text-md">{t('AnimeIdsLead')}</p>
<ErrorBanner message={table.error} onRetry={() => void table.reload()} />
<OutcomeBanner {outcome} />
{#if table.data}
  {@const state = table.data}
  <p class="mt-2">
    {#if state.loaded_at === null}
      {t('AnimeIdsNone')}
    {:else}
      {t('AnimeIdsHeld', {
        count: formatCount(state.entries ?? 0, i18n.language),
        when: formatRelative(state.loaded_at, i18n.language),
      })}
      <span class="text-muted">
        {state.origin === 'file'
          ? t('AnimeIdsFromFile')
          : t('AnimeIdsFrom', { address: state.origin ?? '' })}
      </span>
    {/if}
  </p>
  {#if setting}
    <div class="mt-3">{@render setting()}</div>
  {/if}
  {#if !access.readOnly}
    <div class="flex flex-wrap gap-2 mt-2">
      <button
        type="button"
        class="btn btn-secondary"
        disabled={loading}
        onclick={() => void load(() => api.downloadAnimeIds(following))}
      >
        <Download size={16} />
        {t('AnimeIdsDownload')}
      </button>
      <FileButton
        label={t('AnimeIdsImport')}
        accept="application/json,.json"
        onFile={(file) => void load(() => api.importAnimeIds(file, following))}
      />
    </div>
    <p class="text-muted text-sm mt-1">
      {t('AnimeIdsAddress', { address: state.address })}
    </p>
  {/if}
{/if}
