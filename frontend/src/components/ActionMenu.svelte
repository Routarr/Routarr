<script lang="ts">
  import { MoreHorizontal } from '../lib/icons';
  import type { Snippet } from 'svelte';
  import { t } from '../lib/i18n.svelte';

  interface Action {
    label: string;
    icon?: Snippet;
    danger?: boolean;
    disabled?: boolean;
    onSelect: () => void;
  }

  /**
   * The actions of a row beyond the two it shows, so that a row never runs off the screen.
   *
   * The arrows walk the items, as a screen reader user expects of a `role="menu"`.
   */
  let {
    actions,
    label,
    id,
  }: {
    actions: Action[];
    label?: string;
    /** For a screen that hands the focus here once the row it was in is gone. */
    id?: string;
  } = $props();

  let open = $state(false);
  let root = $state<HTMLDivElement | null>(null);
  let trigger = $state<HTMLButtonElement | null>(null);

  const items = () => [...(root?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];

  function close(returnFocus: boolean) {
    open = false;
    if (returnFocus) trigger?.focus();
  }

  function onMenuKey(event: KeyboardEvent) {
    const entries = items();
    const at = entries.indexOf(document.activeElement as HTMLElement);
    const move = (to: number) => {
      event.preventDefault();
      entries[(to + entries.length) % entries.length]?.focus();
    };
    switch (event.key) {
      case 'ArrowDown':
        move(at + 1);
        break;
      case 'ArrowUp':
        move(at - 1);
        break;
      case 'Home':
        move(0);
        break;
      case 'End':
        move(entries.length - 1);
        break;
      case 'Tab':
        // The menu is one tab stop, and leaving it closes it.
        open = false;
        break;
      default:
    }
  }

  const usable = $derived(actions.filter((action) => !action.disabled));
  const name = $derived(label ?? t('Actions'));

  $effect(() => {
    if (!open) return;
    // A menu button opens with the focus on its first item.
    items()[0]?.focus();

    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close(true);
    };
    const onClick = (event: MouseEvent) => {
      if (!root?.contains(event.target as Node)) open = false;
    };

    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onClick);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onClick);
    };
  });
</script>

{#if usable.length > 0}
  <div class="action-menu" bind:this={root}>
    <button
      {id}
      type="button"
      class="btn btn-ghost btn-sm"
      aria-haspopup="menu"
      aria-expanded={open}
      aria-label={name}
      title={name}
      bind:this={trigger}
      onclick={() => (open = !open)}
      onkeydown={(event) => {
        if (event.key === 'ArrowDown' && !open) {
          event.preventDefault();
          open = true;
        }
      }}
    >
      <MoreHorizontal size={16} aria-hidden="true" />
    </button>

    {#if open}
      <div class="action-menu-list" role="menu" tabindex="-1" onkeydown={onMenuKey}>
        {#each usable as action (action.label)}
          <button
            type="button"
            role="menuitem"
            tabindex="-1"
            class="action-menu-item{action.danger ? ' is-danger' : ''}"
            onclick={() => {
              // Focus the trigger first: a dialog the action opens hands the focus back to it.
              close(true);
              action.onSelect();
            }}
          >
            {#if action.icon}{@render action.icon()}{/if}
            <span>{action.label}</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>
{/if}
