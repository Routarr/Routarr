<script lang="ts">
  import { MoreHorizontal } from '../lib/icons';
  import type { Snippet } from 'svelte';
  import { t } from '../lib/i18n.svelte';

  interface Action {
    label: string;
    icon?: Snippet;
    danger?: boolean;
    disabled?: boolean;
    /** The heading it is listed under, shared with the actions next to it. */
    group?: string;
    onSelect: () => void;
  }

  /**
   * The actions of a row beyond the two it shows, so that a row never runs off the screen, or
   * a choice of things to add behind one button.
   *
   * The arrows walk the items, as a screen reader user expects of a `role="menu"`.
   */
  let {
    actions,
    label,
    id,
    trigger,
    align = 'end',
    describedBy,
  }: {
    actions: Action[];
    label?: string;
    /** For a screen that hands the focus here once the row it was in is gone. */
    id?: string;
    /** What the button shows in place of the ⋯ icon, as a bordered button. */
    trigger?: Snippet;
    /** The side of the button the menu lines up with, `start` for a button opening a line. */
    align?: 'start' | 'end';
    describedBy?: string;
  } = $props();

  const uid = $props.id();
  let open = $state(false);
  let root = $state<HTMLDivElement | null>(null);
  let button = $state<HTMLButtonElement | null>(null);

  const items = () => [...(root?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];

  function close(returnFocus: boolean) {
    open = false;
    if (returnFocus) button?.focus();
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
  const sections = $derived(
    usable.reduce<{ group?: string; actions: Action[] }[]>((runs, action) => {
      const last = runs.at(-1);
      if (last && last.group === action.group) last.actions.push(action);
      else runs.push({ group: action.group, actions: [action] });
      return runs;
    }, []),
  );

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

{#snippet item(action: Action)}
  <button
    type="button"
    role="menuitem"
    tabindex="-1"
    class="action-menu-item{action.danger ? ' is-danger' : ''}"
    onclick={() => {
      // Focus the button first: a dialog the action opens hands the focus back to it.
      close(true);
      action.onSelect();
    }}
  >
    {#if action.icon}{@render action.icon()}{/if}
    <span>{action.label}</span>
  </button>
{/snippet}

{#if usable.length > 0}
  <div class="action-menu" bind:this={root}>
    <button
      {id}
      type="button"
      class={trigger ? 'btn btn-secondary' : 'btn btn-ghost btn-sm'}
      aria-haspopup="menu"
      aria-expanded={open}
      aria-label={trigger ? label : name}
      aria-describedby={describedBy}
      title={trigger ? undefined : name}
      bind:this={button}
      onclick={() => (open = !open)}
      onkeydown={(event) => {
        if (event.key === 'ArrowDown' && !open) {
          event.preventDefault();
          open = true;
        }
      }}
    >
      {#if trigger}
        {@render trigger()}
      {:else}
        <MoreHorizontal size={16} aria-hidden="true" />
      {/if}
    </button>

    {#if open}
      <div
        class="action-menu-list{align === 'start' ? ' is-start' : ''}"
        role="menu"
        tabindex="-1"
        onkeydown={onMenuKey}
      >
        {#each sections as section, index (index)}
          {#if section.group}
            <div role="group" aria-labelledby="{uid}-group-{index}">
              <div class="action-menu-group" id="{uid}-group-{index}" role="presentation">
                {section.group}
              </div>
              {#each section.actions as action (action.label)}{@render item(action)}{/each}
            </div>
          {:else}
            {#each section.actions as action (action.label)}{@render item(action)}{/each}
          {/if}
        {/each}
      </div>
    {/if}
  </div>
{/if}
