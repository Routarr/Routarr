<script lang="ts">
  import { MoreHorizontal } from '../lib/icons';
  import type { Snippet } from 'svelte';
  import { t } from '../lib/i18n.svelte';

  export interface Action {
    label: string;
    icon?: Snippet;
    danger?: boolean;
    disabled?: boolean;
    onSelect: () => void;
  }

  /**
   * The actions a row has beyond the two it shows.
   *
   * Seven buttons in one row overflow a 1440px screen, and the ones past the
   * edge go off it with nothing saying so. No width fixes that; the number of
   * actions does. Two stay visible, the rest live here.
   *
   * Built rather than borrowed because the alternative is a dropdown library for
   * one menu, and the behaviour that matters is small: Escape closes it and
   * hands the focus back, a click outside closes it, the trigger says whether
   * it is open, and the arrows walk it — a `role="menu"` announces a menu, and
   * a screen reader user then reaches for the arrows, not for Tab.
   */
  let { actions, label }: { actions: Action[]; label?: string } = $props();

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
        // The menu is one tab stop; leaving it closes it.
        open = false;
        break;
      default:
    }
  }

  const usable = $derived(actions.filter((action) => !action.disabled));
  const name = $derived(label ?? t('Actions'));

  $effect(() => {
    if (!open) return;
    // Focus follows the menu in: the first item, as a menu button opens.
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
              open = false;
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
