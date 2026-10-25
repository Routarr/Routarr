<script lang="ts">
  import { api } from '../api/client';
  import { runsOf } from '../api/format';
  import { RefreshCw, Search, X } from '../lib/icons';
  import { describeError } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import { folded, searchHelp } from '../lib/help';
  import { composing } from '../lib/keys';
  import { access } from '../lib/access.svelte';
  import { destinationsFor } from '../lib/navigation';
  import { screenKey } from '../lib/routes';
  import { navigate } from '../lib/router.svelte';
  import type { Explanation, MediaListItem } from '../api/types';
  import ExplanationModal from './ExplanationModal.svelte';
  import Modal from './Modal.svelte';

  /**
   * One field, from anywhere, that opens a title's explanation in place, since neither it nor
   * the rule editor has an address. It holds no write to a library: those keep their
   * guardrails on the screens that own them. The help answers here too.
   */
  let { onClose, onHelp }: { onClose: () => void; onHelp: (screen: string) => void } = $props();

  type Row =
    | { kind: 'nav'; id: string; to: string; label: string; hint: string }
    | { kind: 'help'; id: string; screen: string; title: string; where: string }
    | { kind: 'media'; id: string; media: MediaListItem };

  let query = $state('');
  let media = $state<MediaListItem[]>([]);
  /** The library is being asked about what was typed. */
  let searching = $state(false);
  let cursor = $state(0);
  let explaining = $state<Explanation | null>(null);
  let error = $state<string | null>(null);
  let input = $state<HTMLInputElement | null>(null);

  /** The help's first answers, three beside titles found to keep those in view. */
  const help = $derived(
    query.trim().length < 2 ? [] : searchHelp(query, t).slice(0, media.length > 0 ? 3 : 5),
  );

  /** Matched on the translated label: it is what is on screen and what is typed. */
  const destinations = $derived(
    destinationsFor(access.readOnly)
      .map((item) => ({ item, label: t(item.key) }))
      .filter(({ label }) => !query.trim() || folded(label).includes(folded(query.trim()))),
  );

  const rows = $derived<Row[]>([
    ...destinations.map(({ item, label }) => ({
      kind: 'nav' as const,
      id: `pal-nav-${item.to}`,
      to: item.to,
      label,
      hint: t(item.hint),
    })),
    // The titles before the help: finding one and asking why is what the field is for.
    ...media.map((entry) => ({
      kind: 'media' as const,
      id: `pal-media-${entry.id}`,
      media: entry,
    })),
    ...help.map((hit, at) => ({
      kind: 'help' as const,
      id: `pal-help-${at}`,
      screen: hit.screen,
      title: hit.title,
      where: t(screenKey(hit.screen)),
    })),
  ]);

  const current = $derived(rows[Math.min(cursor, rows.length - 1)]);
  /** The rows in runs of one kind, each with its place in the whole list, which the arrows count. */
  const runs = $derived(runsOf(rows, (row) => row.kind));

  // The library is asked once the typing stops, never on the keystroke: this
  // runs against somebody's Radarr host over their own network.
  $effect(() => {
    const term = query.trim();
    if (term.length < 2) {
      media = [];
      error = null;
      searching = false;
      return;
    }
    // A search the typing has moved past is abandoned, not left to run.
    const search = new AbortController();
    const live = () => !search.signal.aborted;
    searching = true;
    const timer = setTimeout(() => {
      api
        .getLibrary({ search: term, per_page: 5 }, search.signal)
        .then((page) => {
          if (live()) ((media = page.data), (error = null));
        })
        .catch((cause) => {
          if (live()) ((media = []), (error = describeError(cause)));
        })
        .finally(() => {
          if (live()) searching = false;
        });
    }, 200);
    return () => {
      search.abort();
      clearTimeout(timer);
    };
  });

  // A new search is a new list, and a cursor left where it was would point at a
  // row that is no longer there.
  $effect(() => {
    void query;
    cursor = 0;
  });

  $effect(() => {
    input?.focus();
  });

  // Only the last title chosen answers, as on the library screen.
  let asking: AbortController | null = null;
  /** The title whose explanation is on its way, which its option says. */
  let asked = $state<string | null>(null);

  async function open(row: Row) {
    if (row.kind === 'nav') {
      navigate(row.to);
      onClose();
      return;
    }
    if (row.kind === 'help') {
      onHelp(row.screen);
      return;
    }
    asking?.abort();
    const mine = (asking = new AbortController());
    asked = row.media.id;
    try {
      error = null;
      const answer = await api.explainTitle(row.media.id, mine.signal);
      if (!mine.signal.aborted) explaining = answer;
    } catch (cause) {
      if (!mine.signal.aborted) error = describeError(cause);
    } finally {
      if (!mine.signal.aborted) asked = null;
    }
  }

  function onKey(event: KeyboardEvent) {
    if (composing(event)) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (rows.length === 0) return;
      const step = event.key === 'ArrowDown' ? 1 : -1;
      cursor = (cursor + step + rows.length) % rows.length;
      // jsdom has no `scrollIntoView`, so the call is optional, as `Modal`'s `showModal` is.
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
  <!-- A click beside it closes it: it holds nothing that a stray click could
       lose, and opening it is not a promise to go somewhere. -->
  <Modal label={t('CommandPalette')} {onClose} maxWidth={520} closeOnBackdrop>
    <div class="palette">
      <div class="palette-field">
        <Search size={16} aria-hidden="true" />
        <!-- The focus never leaves the field, so typing continues while the
             arrows walk the list, and `aria-activedescendant` is what tells a
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
        <!-- The way out a pointer sees, as every other dialog has one.
             Escape is the keyboard's. -->
        <button
          type="button"
          class="close-button"
          onclick={onClose}
          aria-label={t('Dismiss')}
          title={t('Dismiss')}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </div>

      <!-- Announced, because a screen reader hears the field and nothing else:
           the list below it is not where the focus is. -->
      <p class="visually-hidden" role="status">
        {t('CommandPaletteResults', { count: rows.length })}
      </p>

      <!-- Beside the results: the destinations need no network, and stay usable through the
           library's failure. -->
      {#if error}
        <p class="palette-error" role="alert">{error}</p>
      {/if}

      {#if rows.length === 0 && searching}
        <!-- Until the library answers, "nothing for" is a claim it has not made. -->
        <p class="palette-empty">{t('Loading')}</p>
      {:else if rows.length === 0}
        <p class="palette-empty">
          {t('CommandPaletteEmpty', { query: query.trim() })}
          <span>{t('CommandPaletteHint')}</span>
        </p>
      {:else}
        <div
          class="palette-list"
          id="palette-results"
          role="listbox"
          aria-label={t('CommandPalette')}
        >
          <!-- One group per run, named by its heading: "History" the screen and
               "History" the film are otherwise the same option to a screen
               reader. -->
          {#each runs as run (run.key)}
            <div role="group" aria-labelledby="palette-group-{run.key}">
              <div class="palette-group" id="palette-group-{run.key}" role="presentation">
                {t(
                  run.key === 'nav'
                    ? 'CommandPaletteGoTo'
                    : run.key === 'help'
                      ? 'Help'
                      : 'Library',
                )}
              </div>
              {#each run.entries as { item: row, index } (row.id)}
                <!-- The option takes the click itself, since an `option` holds no interactive
                     content. The keyboard path is the field's (arrows and Enter, the combobox
                     pattern), which Svelte's rule cannot see. -->
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <div
                  id={row.id}
                  class="palette-row{row.kind === 'nav' ? ' is-destination' : ''}{index === cursor
                    ? ' is-current'
                    : ''}"
                  role="option"
                  tabindex="-1"
                  aria-selected={index === cursor}
                  aria-busy={row.kind === 'media' && asked === row.media.id}
                  onclick={() => void open(row)}
                >
                  {#if row.kind === 'nav'}
                    <span class="palette-name">{row.label}</span>
                    <span class="palette-hint" title={row.hint}>{row.hint}</span>
                  {:else if row.kind === 'help'}
                    <span class="palette-name">{row.title}</span>
                    <span class="palette-meta">{row.where}</span>
                  {:else}
                    <span class="palette-name">
                      {row.media.title}
                      {#if asked === row.media.id}
                        <RefreshCw size={12} class="spin" aria-hidden="true" />
                      {/if}
                    </span>
                    <span class="palette-meta">
                      {[
                        row.media.year ?? null,
                        row.media.instance_name,
                        row.media.computed_category,
                      ]
                        .filter(Boolean)
                        .join(' · ')}
                    </span>
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </Modal>
{/if}
