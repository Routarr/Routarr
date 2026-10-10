<script lang="ts">
  import { tick } from 'svelte';
  import { HelpCircle } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';
  import { CARD_HELP } from '../lib/help';

  /**
   * The "?" beside a card's title, and the few sentences it opens on what the
   * card shows. `CardTitle` draws it beside its heading, and a title that
   * cannot hold a button, inside a `<summary>`, places it itself.
   *
   * The bubble is a popover, which the browser lays above everything, dialogs
   * included, and closes on Escape or a click elsewhere. Its text is written
   * into it once it opens, so a screen reader announces it as a status.
   */
  let {
    card,
    title,
  }: {
    /** The card's id in `CARD_HELP`, which holds what its "?" says. */
    card: string;
    /** The id of the card's heading, which names the button after the card. */
    title: string;
  } = $props();

  const uid = $props.id();
  let button = $state<HTMLButtonElement>();
  let bubble = $state<HTMLElement>();
  let open = $state(false);
  let placed = $state(false);

  /** Under the "?", or above it near the bottom of the window, and inside the window either way. */
  function place() {
    const anchor = button!.getBoundingClientRect();
    const box = bubble!.getBoundingClientRect();
    const margin = 8;
    const below = anchor.bottom + 6;
    const top =
      below + box.height > innerHeight - margin
        ? Math.max(margin, anchor.top - 6 - box.height)
        : below;
    const start = document.documentElement.dir === 'rtl' ? anchor.right - box.width : anchor.left;
    bubble!.style.top = `${top}px`;
    bubble!.style.left = `${Math.min(Math.max(margin, start), innerWidth - box.width - margin)}px`;
    placed = true;
  }

  async function toggled(event: Event) {
    open = (event as ToggleEvent).newState === 'open';
    placed = false;
    if (!open) return;
    await tick();
    place();
  }

  // A page that scrolls under an open bubble moves its "?" away from it.
  $effect(() => {
    if (!open) return;
    const follow = () => place();
    window.addEventListener('scroll', follow, { capture: true, passive: true });
    window.addEventListener('resize', follow);
    return () => {
      window.removeEventListener('scroll', follow, { capture: true });
      window.removeEventListener('resize', follow);
    };
  });
</script>

<button
  bind:this={button}
  type="button"
  class="help-toggle"
  popovertarget="card-help-{uid}"
  aria-expanded={open}
  aria-labelledby="card-help-label-{uid} {title}"
>
  <HelpCircle size={15} aria-hidden="true" />
</button>
<span id="card-help-label-{uid}" hidden>{t('HelpOn')}</span>
<div
  bind:this={bubble}
  id="card-help-{uid}"
  class="help-bubble"
  class:is-placed={placed}
  popover="auto"
  role="status"
  ontoggle={toggled}
>
  {#if open}{t(CARD_HELP[card]!.text)}{/if}
</div>
