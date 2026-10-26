<script lang="ts" module>
  import { api } from '../api/client';
  import type { ReadFilter } from '../lib/address.svelte';
  import { createAsync } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';

  /** The titles a list of decisions narrows to, each field empty for every value. */
  export interface TitleFilter {
    instance_id: string;
    media_type: string;
    category: string;
    current_root_folder: string;
    rule_id: string;
  }

  export type TitleField = 'instance' | 'type' | 'category' | 'folder' | 'rule';

  export const NO_TITLE_FILTER: TitleFilter = {
    instance_id: '',
    media_type: '',
    category: '',
    current_root_folder: '',
    rule_id: '',
  };

  /** The value each field sets. The address carries each under the field's own name. */
  const SETS: Record<TitleField, keyof TitleFilter> = {
    instance: 'instance_id',
    type: 'media_type',
    category: 'category',
    folder: 'current_root_folder',
    rule: 'rule_id',
  };

  /** The values of `fields` a link names, an unknown type read as every type. */
  export function titlesFromAddress(read: ReadFilter, fields: readonly TitleField[]): TitleFilter {
    const filter = { ...NO_TITLE_FILTER };
    for (const field of fields) {
      filter[SETS[field]] = read(field, field === 'type' ? ['movie', 'series'] : undefined);
    }
    return filter;
  }

  export function titlesInAddress(
    filter: TitleFilter,
    fields: readonly TitleField[],
  ): Record<string, string> {
    return Object.fromEntries(fields.map((field) => [field, filter[SETS[field]]]));
  }

  /** What each field offers, read once for the panel and for a screen that names its filters. */
  export function createFilterOptions(fields: readonly TitleField[]) {
    const shown = (field: TitleField) => fields.includes(field);
    return createAsync(async (signal) => {
      const [instances, categories, folders, rules] = await Promise.all([
        shown('instance') ? api.getInstances(signal) : [],
        shown('category') ? api.getCategories(signal) : [],
        shown('folder') ? api.getRootFolders(signal) : [],
        shown('rule') ? api.getRules(signal) : [],
      ]);
      // Two instances may name one path, and the filter reads the path alone.
      const paths = [...new Set(folders.map((folder) => folder.path))].sort();
      return { instances, categories, paths, rules };
    });
  }

  export type FilterOptions = ReturnType<typeof createFilterOptions>;

  /** Each field set, as its label and the value the panel shows, its id until the names load. */
  export function describeTitles(
    filter: TitleFilter,
    options: FilterOptions,
  ): { label: string; value: string }[] {
    const named = options.data;
    const terms = [
      {
        label: t('Instance'),
        value: filter.instance_id,
        name: named?.instances.find((each) => each.id === filter.instance_id)?.name,
      },
      {
        label: t('Type'),
        value: filter.media_type,
        name: filter.media_type ? t(filter.media_type === 'movie' ? 'Movies' : 'Series') : '',
      },
      { label: t('Category'), value: filter.category, name: filter.category },
      {
        label: t('CurrentRootFolder'),
        value: filter.current_root_folder,
        name: filter.current_root_folder,
      },
      {
        label: t('Rule'),
        value: filter.rule_id,
        name: named?.rules.find((each) => each.id === filter.rule_id)?.name,
      },
    ];
    return terms
      .filter((term) => term.value !== '')
      .map((term) => ({ label: term.label, value: term.name || term.value }));
  }
</script>

<script lang="ts">
  import ErrorBanner from './ErrorBanner.svelte';

  /** The labelled selects of a filter panel that narrow a list of decisions by title. */
  let {
    filter = $bindable(),
    fields,
    onchange,
    options = createFilterOptions(fields),
  }: {
    filter: TitleFilter;
    fields: readonly TitleField[];
    /** Fired after a field changed, where a screen resets its paging. */
    onchange?: () => void;
    /** A screen's own read, shared when it also names the filters set. */
    options?: FilterOptions;
  } = $props();

  const shown = (field: TitleField) => fields.includes(field);
  const id = $props.id();

  function set(field: keyof TitleFilter, value: string) {
    filter = { ...filter, [field]: value };
    onchange?.();
  }

  // A link may name what was deleted since: the list falls back to every value, not to none.
  $effect(() => {
    const named = options.data;
    if (!named) return;
    const offered: Partial<Record<keyof TitleFilter, string[]>> = {
      instance_id: named.instances.map((each) => each.id),
      category: named.categories.map((each) => each.name),
      current_root_folder: named.paths,
      rule_id: named.rules.map((each) => each.id),
    };
    const stale = fields
      .map((field) => SETS[field])
      .filter((key) => filter[key] && offered[key] && !offered[key].includes(filter[key]));
    if (stale.length === 0) return;
    filter = { ...filter, ...Object.fromEntries(stale.map((key) => [key, ''])) };
    onchange?.();
  });
</script>

<ErrorBanner
  message={options.error}
  onDismiss={() => (options.error = null)}
  onRetry={() => void options.reload()}
/>
{#if shown('instance')}
  <div class="form-group filter-field">
    <label class="form-label" for="{id}-instance">{t('Instance')}</label>
    <select
      id="{id}-instance"
      class="form-select"
      value={filter.instance_id}
      onchange={(event) => set('instance_id', event.currentTarget.value)}
    >
      <option value="">{t('AllInstances')}</option>
      {#each options.data?.instances ?? [] as instance (instance.id)}
        <option value={instance.id}>{instance.name}</option>
      {/each}
    </select>
  </div>
{/if}
{#if shown('type')}
  <div class="form-group filter-field">
    <label class="form-label" for="{id}-type">{t('Type')}</label>
    <select
      id="{id}-type"
      class="form-select"
      value={filter.media_type}
      onchange={(event) => set('media_type', event.currentTarget.value)}
    >
      <option value="">{t('AllTypes')}</option>
      <option value="movie">{t('Movies')}</option>
      <option value="series">{t('Series')}</option>
    </select>
  </div>
{/if}
{#if shown('category')}
  <div class="form-group filter-field">
    <label class="form-label" for="{id}-category">{t('Category')}</label>
    <select
      id="{id}-category"
      class="form-select"
      value={filter.category}
      onchange={(event) => set('category', event.currentTarget.value)}
    >
      <option value="">{t('AllCategories')}</option>
      {#each options.data?.categories ?? [] as category (category.id)}
        <option value={category.name}>{category.name}</option>
      {/each}
    </select>
  </div>
{/if}
{#if shown('folder')}
  <div class="form-group filter-field">
    <label class="form-label" for="{id}-folder">{t('CurrentRootFolder')}</label>
    <select
      id="{id}-folder"
      class="form-select"
      value={filter.current_root_folder}
      onchange={(event) => set('current_root_folder', event.currentTarget.value)}
    >
      <option value="">{t('AllCurrentFolders')}</option>
      {#each options.data?.paths ?? [] as path (path)}
        <option value={path}>{path}</option>
      {/each}
    </select>
  </div>
{/if}
{#if shown('rule')}
  <div class="form-group filter-field">
    <label class="form-label" for="{id}-rule">{t('Rule')}</label>
    <select
      id="{id}-rule"
      class="form-select"
      value={filter.rule_id}
      onchange={(event) => set('rule_id', event.currentTarget.value)}
    >
      <option value="">{t('AllRules')}</option>
      {#each options.data?.rules ?? [] as rule (rule.id)}
        <option value={rule.id}>{rule.name}</option>
      {/each}
    </select>
  </div>
{/if}
