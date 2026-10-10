<script lang="ts">
  import type { Snippet } from 'svelte';
  import { AlertTriangle } from '../lib/icons';

  /**
   * Findings of one severity as one banner: the count leads, the list follows,
   * and an action, where there is one, sits once beside the count. One banner
   * per finding would say the severity once per line, each with its own stripe.
   * Without a title, the list stands under a headline said elsewhere.
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
            {#if item.lead}<strong>{item.lead}: </strong>{/if}{item.text}
          </li>
        {/each}
      </ul>
    </div>
  </div>
{/if}
