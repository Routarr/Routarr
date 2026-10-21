<script lang="ts">
  import CardTitle from './CardTitle.svelte';
  import { ApiError, api } from '../api/client';
  import type { Category, FolderSuggestion, TemplateOffer } from '../api/types';
  import { describeError } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { href } from '../lib/router.svelte';
  import { invalidateStatus } from '../lib/status.svelte';

  /**
   * The categories the folders' names suggest, mapped on a click and undone on another. Nothing
   * is written before a click, and a category a map created goes with its undo.
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
    /** The category's id when this map created it, for the undo to remove. */
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
    let created: string | null = null;
    if (!categories.some((known) => known.name === category)) {
      try {
        created = (await api.createCategory({ name: category })).id;
      } catch (cause) {
        // Made meanwhile, by a row mapped just before on another instance: it is used as is.
        if (!(cause instanceof ApiError && cause.status === 409)) throw cause;
      }
    }
    try {
      await api.updateRootFolderCategory(each.root_folder_id, category);
    } catch (cause) {
      // The refusal said is the mapping's: a category left behind would name itself in a warning.
      if (created) await api.deleteCategory(created).catch(() => undefined);
      throw cause;
    }
    const { root_folder_id: folder, path, instance_name: instance, template } = each;
    return { folder, path, instance, category, created, template };
  }

  async function settle() {
    invalidateStatus();
    await onChanged();
    busy = false;
  }

  async function map(each: FolderSuggestion) {
    busy = true;
    try {
      const row = await mapOne(each);
      mapped = [...mapped, row];
      outcome.succeed(t('FolderSuggestionMapped', { path: row.path, category: row.category }));
    } catch (cause) {
      outcome.fail(cause);
    }
    await settle();
  }

  async function mapAll() {
    busy = true;
    const failed: string[] = [];
    let count = 0;
    for (const each of ready) {
      try {
        mapped = [...mapped, await mapOne(each)];
        count += 1;
      } catch (cause) {
        failed.push(`${each.path}: ${describeError(cause)}`);
      }
    }
    if (failed.length > 0) outcome.warn(t('FolderSuggestionsPartly', { count }), failed);
    else outcome.succeed(t('FolderSuggestionsMapped', { count }));
    await settle();
  }

  async function undo(row: Mapped) {
    busy = true;
    try {
      await api.updateRootFolderCategory(row.folder, null);
      if (row.created) await api.deleteCategory(row.created);
      mapped = mapped.filter((other) => other !== row);
      outcome.succeed(t('FolderSuggestionUndone', { path: row.path }));
    } catch (cause) {
      outcome.fail(cause);
    }
    await settle();
  }

  async function setDismissed(each: FolderSuggestion, dismissed: boolean) {
    busy = true;
    try {
      if (dismissed) await api.dismissFolderSuggestion(each.root_folder_id);
      else await api.restoreFolderSuggestion(each.root_folder_id);
    } catch (cause) {
      outcome.fail(cause);
    }
    await settle();
  }

  const rulesOf = (row: Mapped, offer: TemplateOffer) =>
    href(`/rules?template=${offer.id}&category=${encodeURIComponent(row.category)}`);
</script>

{#if shown.length > 0 || mapped.length > 0 || dismissedCount > 0}
  <section class="card" aria-labelledby="folder-suggestions-title">
    <div class="card-header">
      <CardTitle card="FolderSuggestions" id="folder-suggestions-title">
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
      {#each mapped as row (row.folder)}
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
              class="btn btn-ghost btn-sm"
              disabled={busy}
              aria-label="{t('FolderSuggestionUndo')} – {row.path}"
              onclick={() => void undo(row)}
            >
              {t('FolderSuggestionUndo')}
            </button>
          </div>
        </li>
      {/each}
      {#each shown as each (each.root_folder_id)}
        <li class="suggestion-row" class:is-dismissed={each.dismissed}>
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
                class="btn btn-ghost btn-sm"
                disabled={busy}
                aria-label="{t('FolderSuggestionRestore')} – {each.path}"
                onclick={() => void setDismissed(each, false)}
              >
                {t('FolderSuggestionRestore')}
              </button>
            {:else}
              <button
                class="btn btn-secondary btn-sm"
                disabled={busy || choiceOf(each) === ''}
                aria-label="{t('FolderSuggestionMap')} – {each.path}"
                onclick={() => void map(each)}
              >
                {t('FolderSuggestionMap')}
              </button>
              <button
                class="btn btn-ghost btn-sm"
                disabled={busy}
                aria-label="{t('FolderSuggestionDismiss')} – {each.path}"
                onclick={() => void setDismissed(each, true)}
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
