<script lang="ts">
  import type { Paginated } from '../api/types';
  import { handFocus, type FocusTarget } from '../lib/focus';
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

  let previous = $state<HTMLButtonElement>();
  let next = $state<HTMLButtonElement>();

  /** The pressed button turns disabled at an end, so the focus moves to the other one. */
  function step(by: number, pressed: FocusTarget, other: FocusTarget) {
    page += by;
    void handFocus(pressed, other);
  }
</script>

{#if pagination && pagination.total_pages > 1}
  <div class="flex items-center justify-between mt-4">
    <span class="text-muted text-md">
      {t('PageOf', { page: pagination.page, total: pagination.total_pages })} · {t(countKey, {
        count: pagination.total,
      })}
    </span>
    <div class="flex gap-2">
      <button
        bind:this={previous}
        class="btn btn-secondary btn-sm"
        disabled={page <= 1}
        onclick={() => step(-1, previous, next)}
      >
        {t('Previous')}
      </button>
      <button
        bind:this={next}
        class="btn btn-secondary btn-sm"
        disabled={page >= pagination.total_pages}
        onclick={() => step(1, next, previous)}
      >
        {t('Next')}
      </button>
    </div>
  </div>
{/if}
