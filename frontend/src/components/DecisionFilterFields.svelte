<script lang="ts" module>
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
</script>

<script lang="ts">
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  /** The labelled selects of a filter panel that narrow a list of decisions by title. */
  let {
    filter = $bindable(),
    fields,
    onchange,
  }: {
    filter: TitleFilter;
    fields: readonly TitleField[];
    /** Fired after a field changed, where a screen resets its paging. */
    onchange?: () => void;
  } = $props();

  const shown = (field: TitleField) => fields.includes(field);
  const id = $props.id();

  const options = createAsync(async (signal) => {
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

  function set(field: keyof TitleFilter, value: string) {
    filter = { ...filter, [field]: value };
    onchange?.();
  }
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
