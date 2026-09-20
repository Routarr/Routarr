<script lang="ts">
  import { api } from '../api/client';
  import { describeError } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import { DESTINATIONS } from '../lib/navigation';
  import { navigate } from '../lib/router.svelte';
  import type { Explanation, MediaListItem } from '../api/types';
  import ExplanationModal from './ExplanationModal.svelte';
  import Modal from './Modal.svelte';

  /**
   * One field, from anywhere, for the question this product exists to answer.
   *
   * "Why did Routarr put this film there?" took four steps from any screen:
   * open the library, type, search, then find the row and press its button.
   * The answer arrives here without leaving the page — the palette fetches the
   * explanation itself rather than navigating to a screen that would show it,
   * because neither the explanation nor the rule editor is addressable by URL
   * and sending someone to `/media` would only be step one of the four again.
   *
   * Deliberately narrow. It carries no action that writes: applying a move,
   * reverting one and deleting a rule all have guardrails that live on the
   * screen owning them — a confirmation threshold, a batch ceiling, a capacity
   * check — and a palette exists to be fast, which is the opposite of what a
   * write to somebody's library wants.
   */
  let { onClose }: { onClose: () => void } = $props();

  type Row =
    | { kind: 'nav'; id: string; to: string; label: string }
    | { kind: 'media'; id: string; media: MediaListItem };

  let query = $state('');
  let media = $state<MediaListItem[]>([]);
  let cursor = $state(0);
  let explaining = $state<Explanation | null>(null);
  let error = $state<string | null>(null);
  let input = $state<HTMLInputElement | null>(null);

  /** Matched on the translated label: it is what is on screen and what is typed. */
  const destinations = $derived(
    DESTINATIONS.map((item) => ({ item, label: t(item.key) })).filter(
      ({ label }) => !query.trim() || label.toLowerCase().includes(query.trim().toLowerCase()),
    ),
  );

  const rows = $derived<Row[]>([
    ...destinations.map(({ item, label }) => ({
      kind: 'nav' as const,
      id: `pal-nav-${item.to}`,
      to: item.to,
      label,
    })),
    ...media.map((entry) => ({
      kind: 'media' as const,
      id: `pal-media-${entry.id}`,
      media: entry,
    })),
  ]);

  const current = $derived(rows[Math.min(cursor, rows.length - 1)]);

  // The library is asked once the typing stops, never on the keystroke: this
  // runs against somebody's Radarr host over their own network.
  $effect(() => {
    const term = query.trim();
    if (term.length < 2) {
      media = [];
      error = null;
      return;
    }
    let live = true;
    const timer = setTimeout(() => {
      api
        .getMedia({ search: term, per_page: 5 })
        .then((page) => {
          if (live) ((media = page.data), (error = null));
        })
        .catch((cause) => {
          if (live) ((media = []), (error = describeError(cause)));
        });
    }, 200);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  });

  // A new search is a new list; leaving the cursor where it was would leave it
  // pointing at a row that is no longer there.
  $effect(() => {
    void query;
    cursor = 0;
  });

  $effect(() => {
    input?.focus();
  });

  async function open(row: Row) {
    if (row.kind === 'nav') {
      navigate(row.to);
      onClose();
      return;
    }
    try {
      error = null;
      explaining = await api.explainMedia(row.media.id);
    } catch (cause) {
      error = describeError(cause);
    }
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (rows.length === 0) return;
      const step = event.key === 'ArrowDown' ? 1 : -1;
      cursor = (cursor + step + rows.length) % rows.length;
      // Called optionally for the same reason `Modal` guards `showModal`:
      // jsdom ships the element and not the method, so a bare call throws
      // out of the handler and every unit test walking the list reports an
      // unhandled error beside its passing assertions.
      document.getElementById(rows[cursor]!.id)?.scrollIntoView?.({ block: 'nearest' });
    } else if (event.key === 'Enter') {
      event.preventDefault();
      if (current) void open(current);
    }
  }
</script>

{#if explaining}
  <!-- The answer replaces the question rather than stacking on it: two dialogs
       deep, Escape becomes ambiguous and the palette behind is unreadable. -->
  <ExplanationModal data={explaining} {onClose} />
{:else}
  <Modal label={t('CommandPalette')} {onClose} maxWidth={520}>
    <div class="palette">
      <div class="palette-field">
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          aria-hidden="true"
        >
          <circle cx="11" cy="11" r="7" />
          <path d="m20 20-3.5-3.5" />
        </svg>
        <!-- The focus never leaves the field, so typing continues while the
             arrows walk the list; `aria-activedescendant` is what tells a
             screen reader which row that is. -->
        <input
          bind:this={input}
          bind:value={query}
          onkeydown={onKey}
          class="palette-input"
          type="text"
          role="combobox"
          autocomplete="off"
          aria-expanded={rows.length > 0}
          aria-controls={rows.length > 0 ? 'palette-results' : undefined}
          aria-autocomplete="list"
          aria-activedescendant={current?.id}
          aria-label={t('CommandPalette')}
          placeholder={t('CommandPalettePlaceholder')}
        />
      </div>

      <!-- Announced, because a screen reader hears the field and nothing else:
           the list below it is not where the focus is. -->
      <p class="visually-hidden" role="status">
        {t('CommandPaletteResults', { count: rows.length })}
      </p>

      <!-- Beside the results, never instead of them. Only the library needs
           the network; the destinations are in memory, and replacing them
           with the failure left the field able to do nothing at all until it
           was closed and opened again. -->
      {#if error}
        <p class="palette-error" role="alert">{error}</p>
      {/if}

      {#if rows.length === 0}
        <p class="palette-empty">
          {t('CommandPaletteEmpty', { query: query.trim() })}
          <span>{t('CommandPaletteHint')}</span>
        </p>
      {:else}
        <ul
          class="palette-list"
          id="palette-results"
          role="listbox"
          aria-label={t('CommandPalette')}
        >
          {#each rows as row, index (row.id)}
            {@const group = index === 0 || rows[index - 1]?.kind !== row.kind}
            {#if group}
              <li class="palette-group" role="presentation">
                {t(row.kind === 'nav' ? 'CommandPaletteGoTo' : 'MediaExplorer')}
              </li>
            {/if}
            <!-- The option carries the click itself rather than wrapping a
                 button: an `option` must not contain interactive content, and
                 a nested button would also be a tab stop — which the combobox
                 pattern cannot have, since the focus stays in the field and
                 `aria-activedescendant` is what says where the arrows are. -->
            <!-- The keyboard path is the field's own: arrows move the cursor
                 and Enter opens what it points at, which is the combobox
                 pattern. Svelte's rule looks for a handler on this element and
                 cannot see one three lines up, so a handler added here to
                 satisfy it would be a second, unreachable path. -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <li
              id={row.id}
              class="palette-row{index === cursor ? ' is-current' : ''}"
              role="option"
              aria-selected={index === cursor}
              onclick={() => void open(row)}
            >
              {#if row.kind === 'nav'}
                <span class="palette-name">{row.label}</span>
              {:else}
                <span class="palette-name">{row.media.title}</span>
                <span class="palette-meta">
                  {[row.media.year ?? null, row.media.instance_name, row.media.computed_category]
                    .filter(Boolean)
                    .join(' · ')}
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </Modal>
{/if}
