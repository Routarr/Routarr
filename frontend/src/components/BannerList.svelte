<script lang="ts">
  import type { Snippet } from 'svelte';
  import { AlertTriangle } from '../lib/icons';
  import { cut } from '../lib/i18n.svelte';
  import { href } from '../lib/router.svelte';

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
    /** `lead` names what a finding is about, in bold before it, and `note` and `link` follow it. */
    items: {
      lead?: string | null;
      text: string;
      note?: string | null;
      link?: { to: string; label: string } | null;
    }[];
    action?: Snippet;
  } = $props();

  // The colon as the language writes it, the lead in bold before it.
  const [before, between, after] = $derived(cut('FilterTerm', ['label', 'value']));
  // Svelte trims the whitespace a block starts with.
  const SPACE = ' ';
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
              >{item.text}{after}{:else}{item.text}{/if}{#if item.note}{SPACE}<span
                class="text-muted">{item.note}</span
              >{/if}{#if item.link}{SPACE}<a class="text-link" href={href(item.link.to)}
                >{item.link.label}</a
              >{/if}
          </li>
        {/each}
      </ul>
    </div>
  </div>
{/if}
