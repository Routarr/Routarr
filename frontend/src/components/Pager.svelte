<script lang="ts">
  import type { Paginated } from '../api/types';
  import { t } from '../lib/i18n.svelte';

  /**
   * The pages of a list the server pages, under its table. `page` is what the
   * screen asks the server for, so the pager moves it and the screen reloads.
   * Nothing is drawn for a list that fits on one page.
   */
  let {
    pagination,
    page = $bindable(),
    countKey,
  }: {
    pagination: Paginated<unknown>['pagination'] | undefined;
    page: number;
    /** The dictionary key counting the whole list, given `{count}`. */
    countKey: string;
  } = $props();
</script>

{#if pagination && pagination.total_pages > 1}
  <div class="flex items-center justify-between mt-4">
    <span class="text-muted text-md">
      {t('PageOf', { page: pagination.page, total: pagination.total_pages })} · {t(countKey, {
        count: pagination.total,
      })}
    </span>
    <div class="flex gap-2">
      <button class="btn btn-secondary btn-sm" disabled={page <= 1} onclick={() => (page -= 1)}>
        {t('Previous')}
      </button>
      <button
        class="btn btn-secondary btn-sm"
        disabled={page >= pagination.total_pages}
        onclick={() => (page += 1)}
      >
        {t('Next')}
      </button>
    </div>
  </div>
{/if}
