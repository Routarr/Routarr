<script lang="ts">
  import type { Snippet } from 'svelte';
  import { AlertTriangle } from '../lib/icons';
  import { cut } from '../lib/i18n.svelte';

  /**
   * Findings of one severity as one banner: the count, the list, and an action once beside the
   * count. Without a title, the list stands under a headline said elsewhere.
   */
  let {
    tone,
    title,
    items,
    action,
  }: {
    tone: 'warning' | 'danger';
    title?: string;
    /** `lead` is what a finding is about, set in bold before it. */
    items: { lead?: string | null; text: string }[];
    action?: Snippet;
  } = $props();

  // The colon as the language writes it, the lead in bold before it.
  const [before, between, after] = $derived(cut('FilterTerm', ['label', 'value']));
</script>

{#if items.length > 0}
  <div class="banner items-start {tone === 'danger' ? 'banner-danger' : 'banner-warning'}">
    <AlertTriangle size={16} />
    <div class="flex-1">
      {#if title || action}
        <div class="flex items-center justify-between gap-2">
          <strong>{title}</strong>
          {@render action?.()}
        </div>
      {/if}
      <ul class="banner-list">
        {#each items as item, index (index)}
          <li>
            {#if item.lead}{before}<strong>{item.lead}{between}</strong
              >{item.text}{after}{:else}{item.text}{/if}
          </li>
        {/each}
      </ul>
    </div>
  </div>
{/if}
