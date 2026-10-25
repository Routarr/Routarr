<script lang="ts">
  import CardTitle from './CardTitle.svelte';
  import { ApiError, api } from '../api/client';
  import type { Category, FolderSuggestion, TemplateOffer } from '../api/types';
  import { describeError } from '../lib/async.svelte';
  import { handFocus } from '../lib/focus';
  import { t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { href } from '../lib/router.svelte';
  import { invalidateStatus } from '../lib/status.svelte';

  /**
   * The categories the folders' names suggest, mapped on a click and undone on another. Nothing
   * is written before a click, and a category a map created goes with the last undo of the rows
   * mapped to it.
   */
  let {
    suggestions,
    categories,
    outcome,
    onChanged,
  }: {
    suggestions: FolderSuggestion[];
    categories: Category[];
    outcome: Outcome;
    /** Reads the folders, the categories and the suggestions again. */
    onChanged: () => Promise<void>;
  } = $props();

  interface Mapped {
    folder: string;
    path: string;
    instance: string;
    category: string;
    /** The category's id when this map or one before it created it, for an undo to remove. */
    created: string | null;
    template: TemplateOffer | null;
  }

  let mapped = $state<Mapped[]>([]);
  let showDismissed = $state(false);
  let busy = $state(false);
  /** The category picked in a row, by folder id, where the suggestion's own is not wanted. */
  const picked = $state<Record<string, string>>({});
  const choiceOf = (each: FolderSuggestion) => picked[each.root_folder_id] ?? each.category ?? '';

  const shown = $derived(suggestions.filter((each) => showDismissed || !each.dismissed));
  const dismissedCount = $derived(suggestions.filter((each) => each.dismissed).length);
  const ready = $derived(shown.filter((each) => !each.dismissed && choiceOf(each) !== ''));

  /** Creates the category when the library has none by that name, then maps the folder. */
  async function mapOne(each: FolderSuggestion): Promise<Mapped> {
    const category = choiceOf(each);
    let made: string | null = null;
    let shared: string | null = null;
    if (!categories.some((known) => known.name === category)) {
      try {
        made = (await api.createCategory({ name: category })).id;
      } catch (cause) {
        // Made meanwhile, as by a row mapped just before on another instance, whose undo it shares.
        if (!(cause instanceof ApiError && cause.status === 409)) throw cause;
        shared = mapped.find((row) => row.category === category)?.created ?? null;
      }
    }
    try {
      await api.updateRootFolderCategory(each.root_folder_id, category);
    } catch (cause) {
      // The refusal said is the mapping's: a category left behind would name itself in a warning.
      if (made) await api.deleteCategory(made).catch(() => undefined);
      throw cause;
    }
    const { root_folder_id: folder, path, instance_name: instance, template } = each;
    return { folder, path, instance, category, created: made ?? shared, template };
  }

  /** A row's button by its action and place, so the focus can land on the row now in its place. */
  const actionId = (action: string, index: number) => `suggestion-${action}-${index}`;

  /**
   * Once the list is read again, the focus goes back to the pressed button, else to the same
   * action on the row now in its place or the one before, else to the card's title.
   */
  async function settle(pressed: Element | null, ...fallbacks: string[]) {
    invalidateStatus();
    await onChanged();
    busy = false;
    void handFocus(pressed as HTMLElement | null, ...fallbacks, 'folder-suggestions-title');
  }

  const near = (action: string, index: number) => [
    actionId(action, index),
    actionId(action, index - 1),
  ];

  async function map(each: FolderSuggestion, index: number) {
    const pressed = document.activeElement;
    busy = true;
    try {
      const row = await mapOne(each);
      mapped = [...mapped, row];
      outcome.succeed(t('FolderSuggestionMapped', { path: row.path, category: row.category }));
    } catch (cause) {
      outcome.fail(cause);
    }
    await settle(pressed, ...near('map', index));
  }

  async function mapAll() {
    const pressed = document.activeElement;
    busy = true;
    const failed: string[] = [];
    let count = 0;
    for (const each of ready) {
      try {
        mapped = [...mapped, await mapOne(each)];
        count += 1;
      } catch (cause) {
        failed.push(t('FilterTerm', { label: each.path, value: describeError(cause) }));
      }
    }
    if (failed.length > 0) outcome.warn(t('FolderSuggestionsPartly', { count }), failed);
    else outcome.succeed(t('FolderSuggestionsMapped', { count }));
    await settle(pressed);
  }

  /** Removes the category the map created unless another mapped folder still uses it. */
  async function release(row: Mapped) {
    if (!row.created) return;
    try {
      await api.deleteCategory(row.created);
    } catch (cause) {
      if (!(cause instanceof ApiError && cause.reason === 'in_use')) throw cause;
    }
  }

  async function undo(row: Mapped, index: number) {
    const pressed = document.activeElement;
    busy = true;
    try {
      await api.updateRootFolderCategory(row.folder, null);
      await release(row);
      mapped = mapped.filter((other) => other !== row);
      outcome.succeed(t('FolderSuggestionUndone', { path: row.path }));
    } catch (cause) {
      outcome.fail(cause);
    }
    await settle(pressed, ...near('undo', index));
  }

  async function setDismissed(each: FolderSuggestion, dismissed: boolean, index: number) {
    const pressed = document.activeElement;
    busy = true;
    try {
      if (dismissed) await api.dismissFolderSuggestion(each.root_folder_id);
      else await api.restoreFolderSuggestion(each.root_folder_id);
    } catch (cause) {
      outcome.fail(cause);
    }
    // The row's other toggle first: shown dismissed, a dismissed row keeps its place.
    const other = dismissed ? 'restore' : 'dismiss';
    await settle(
      pressed,
      actionId(other, index),
      ...near(dismissed ? 'dismiss' : 'restore', index),
    );
  }

  const rulesOf = (row: Mapped, offer: TemplateOffer) =>
    href(`/rules?template=${offer.id}&category=${encodeURIComponent(row.category)}`);
</script>

{#if shown.length > 0 || mapped.length > 0 || dismissedCount > 0}
  <section class="card" aria-labelledby="folder-suggestions-title">
    <div class="card-header">
      <CardTitle card="FolderSuggestions" id="folder-suggestions-title" tabindex={-1}>
        {t('FolderSuggestions')}
      </CardTitle>
      {#if ready.length > 1}
        <button class="btn btn-primary btn-sm" disabled={busy} onclick={() => void mapAll()}>
          {t('FolderSuggestionMapAll')}
        </button>
      {/if}
    </div>
    <p class="text-muted text-base">{t('FolderSuggestionsLead')}</p>
    <ul class="suggestion-list">
      {#each mapped as row, index (row.folder)}
        <li class="suggestion-row is-mapped">
          <div class="suggestion-what">
            <span class="mono">{row.path}</span>
            <span class="text-muted text-sm">{row.instance}</span>
            <span class="status-reason"
              >{t('FolderSuggestionMappedTo', { category: row.category })}</span
            >
          </div>
          <div class="suggestion-actions">
            {#if row.template}
              <a
                class="btn btn-secondary btn-sm"
                href={rulesOf(row, row.template)}
                aria-label="{t('FolderSuggestionAddRules', {
                  name: row.template.name,
                })} – {row.path}"
              >
                {t('FolderSuggestionAddRules', { name: row.template.name })}
              </a>
            {/if}
            <button
              id={actionId('undo', index)}
              class="btn btn-ghost btn-sm"
              disabled={busy}
              aria-label="{t('FolderSuggestionUndo')} – {row.path}"
              onclick={() => void undo(row, index)}
            >
              {t('FolderSuggestionUndo')}
            </button>
          </div>
        </li>
      {/each}
      {#each shown as each, index (each.root_folder_id)}
        <li class="suggestion-row">
          <div class="suggestion-what">
            <span class="mono">{each.path}</span>
            <span class="text-muted text-sm">{each.instance_name}</span>
            <span class="status-reason">{each.reason}</span>
          </div>
          <div class="suggestion-actions">
            <select
              id="suggestion-{each.root_folder_id}-category"
              class="form-select"
              aria-label={t('CategoryForFolder', { path: each.path })}
              value={choiceOf(each)}
              onchange={(event) => (picked[each.root_folder_id] = event.currentTarget.value)}
            >
              {#if each.category === null}
                <option value="">{t('FolderSuggestionPick')}</option>
              {/if}
              {#each each.choices as choice (choice)}
                <option value={choice}>{choice}</option>
              {/each}
            </select>
            {#if each.dismissed}
              <button
                id={actionId('restore', index)}
                class="btn btn-ghost btn-sm"
                disabled={busy}
                aria-label="{t('FolderSuggestionRestore')} – {each.path}"
                onclick={() => void setDismissed(each, false, index)}
              >
                {t('FolderSuggestionRestore')}
              </button>
            {:else}
              <button
                id={actionId('map', index)}
                class="btn btn-secondary btn-sm"
                disabled={busy || choiceOf(each) === ''}
                aria-label="{t('FolderSuggestionMap')} – {each.path}"
                onclick={() => void map(each, index)}
              >
                {t('FolderSuggestionMap')}
              </button>
              <button
                id={actionId('dismiss', index)}
                class="btn btn-ghost btn-sm"
                disabled={busy}
                aria-label="{t('FolderSuggestionDismiss')} – {each.path}"
                onclick={() => void setDismissed(each, true, index)}
              >
                {t('FolderSuggestionDismiss')}
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
    {#if dismissedCount > 0}
      <div class="suggestion-foot">
        <button class="btn btn-ghost btn-sm" onclick={() => (showDismissed = !showDismissed)}>
          {showDismissed
            ? t('FolderSuggestionsHideDismissed')
            : t('FolderSuggestionsShowDismissed', { count: dismissedCount })}
        </button>
      </div>
    {/if}
  </section>
{/if}
