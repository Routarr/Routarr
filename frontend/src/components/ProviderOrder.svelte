<script lang="ts">
  import { ArrowDown, ArrowUp } from '../lib/icons';
  import type { MetadataProvider } from '../api/types';
  import { t } from '../lib/i18n.svelte';
  import { SOURCE_KEY_SETTING } from '../lib/settings';

  let {
    id,
    catalogue,
    value,
    onChange,
    keys,
    onKeyChange,
  }: {
    id: string;
    catalogue: MetadataProvider[];
    value: string;
    onChange: (value: string) => void;
    /**
     * The stored credentials, read so the field shows what the draft holds.
     *
     * Read and not written: the row reports an edit through `onKeyChange`, the
     * way it reports a reordering through `onChange`. Writing into the record
     * instead is an ownership violation Svelte flags in development, and it
     * worked only because the one caller passed a `$state` proxy — handed an
     * ordinary object it would have written into nothing, silently.
     */
    keys?: Record<string, string>;
    /**
     * Given with `keys` or not at all: the field is rendered only where both
     * are, since one without the other is a field that takes a key and drops
     * it.
     */
    onKeyChange?: (setting: string, value: string) => void;
  } = $props();

  const enabled = $derived(
    value
      .split(',')
      .map((entry) => entry.trim())
      .filter((entry) => catalogue.some((provider) => provider.id === entry)),
  );
  const disabled = $derived(catalogue.filter((provider) => !enabled.includes(provider.id)));

  const emit = (ids: string[]) => onChange(ids.join(','));

  function move(index: number, by: number) {
    const next = [...enabled];
    const target = index + by;
    if (target < 0 || target >= next.length) return;
    const from = next[index];
    const to = next[target];
    if (!from || !to) return;
    next[index] = to;
    next[target] = from;
    emit(next);
  }

  /** A source that is listed but cannot answer: it is waiting for a key. */
  const inert = (provider: MetadataProvider) => provider.needs_key && !provider.configured;
</script>

{#snippet credential(provider: MetadataProvider, setting: string)}
  <!-- The credential, in the row of the source it unlocks. Paste and enable
       without leaving the line, and one save covers both.

       Rendered whether or not the source is switched on, and whether or not a
       key is already stored: this is the only field for it anywhere, so
       hiding it once the source works leaves a leaked key impossible to
       rotate and a stored one impossible to replace. The placeholder is what
       says which of the two situations the reader is in. -->
  <div class="source-key">
    <label class="visually-hidden" for="setting-{setting}">
      {provider.display_name}
    </label>
    <input
      id="setting-{setting}"
      aria-describedby="setting-{setting}-help"
      class="form-input"
      type="password"
      autocomplete="off"
      placeholder={provider.configured
        ? t('SecretConfiguredPlaceholder')
        : provider.key_env
          ? t('ProviderKeyOrEnv', { variable: provider.key_env })
          : t('ProviderKeyPlaceholder')}
      value={keys?.[setting] ?? ''}
      oninput={(event) => onKeyChange?.(setting, event.currentTarget.value)}
    />
  </div>
{/snippet}

{#snippet describe(provider: MetadataProvider)}
  {#if provider.needs_key && !provider.configured}
    <!-- The variable is named once, by the field below that accepts it — it
         was written here as well, so every keyless source said it twice. -->
    {t('ProviderNeedsKey')}
  {:else}
    {provider.needs_key ? '' : `${t('ProviderNoKeyNeeded')} · `}{provider.fields.join(', ')}
  {/if}
{/snippet}

<div {id}>
  <!-- Two named groups. Run together as one list, nothing says where "active,
       in priority order" ends and "available" begins — and the available rows
       carry no number, which breaks the column. -->
  <p class="source-group">{t('SourcesActive')}</p>
  <div class="source-list">
    {#each enabled as providerId, index (providerId)}
      {@const provider = catalogue.find((entry) => entry.id === providerId)}
      {#if provider}
        {@const inactive = inert(provider)}
        {@const setting = SOURCE_KEY_SETTING[providerId]}
        <div class="source-row{inactive ? ' waiting' : ''}">
          <span class="source-rank">{index + 1}</span>
          <div>
            <!-- The badge is a sibling, not a child: `.source-name` holds the
                 name and nothing else, so reading it gives the name. -->
            <div class="source-heading">
              <span class="source-name">{provider.display_name}</span>
              <!-- Stated in words, not only by a tint: a source listed without
                   its key answers nothing, and that is a warning rather than a
                   shade of grey. -->
              {#if inactive}
                <span class="badge badge-warning">{t('ProviderInactive')}</span>
              {/if}
            </div>
            <div class="source-detail" id={setting ? `setting-${setting}-help` : undefined}>
              {@render describe(provider)}
            </div>
            {#if setting && keys && onKeyChange}
              {@render credential(provider, setting)}
            {/if}
          </div>
          <div class="source-actions">
            <button
              type="button"
              class="btn btn-secondary btn-sm"
              aria-label="{t('MoveUp')} {provider.display_name}"
              disabled={index === 0}
              onclick={() => move(index, -1)}
            >
              <ArrowUp size={14} />
            </button>
            <button
              type="button"
              class="btn btn-secondary btn-sm"
              aria-label="{t('MoveDown')} {provider.display_name}"
              disabled={index === enabled.length - 1}
              onclick={() => move(index, 1)}
            >
              <ArrowDown size={14} />
            </button>
            <!-- The Arr's own metadata is always read: it costs no request
                 and every genre rule leans on it. -->
            {#if providerId !== 'arr'}
              <button
                type="button"
                class="btn btn-secondary btn-sm"
                onclick={() => emit(enabled.filter((entry) => entry !== providerId))}
              >
                {t('DisableSource')}
              </button>
            {/if}
          </div>
        </div>
      {/if}
    {/each}
  </div>

  <!-- "Available" named exactly the sources that are not: a keyless one can do
       nothing at all. What the group holds is everything switched off, some of
       it waiting for a credential. -->
  <p class="source-group">{t('SourcesInactive')}</p>
  <div class="source-list">
    {#each disabled as provider (provider.id)}
      {@const unusable = inert(provider)}
      {@const setting = SOURCE_KEY_SETTING[provider.id]}
      <div class="source-row waiting">
        <span class="source-rank"></span>
        <div>
          <div class="source-heading">
            <span class="source-name">{provider.display_name}</span>
          </div>
          <div class="source-detail" id={setting ? `setting-${setting}-help` : undefined}>
            {@render describe(provider)}
          </div>

          {#if setting && keys && onKeyChange}
            {@render credential(provider, setting)}
          {/if}
        </div>
        <div class="source-actions">
          <button
            type="button"
            class="btn btn-secondary btn-sm"
            /* The affordance is refused rather than the mistake explained
               afterwards: enabling it would change nothing at all. */
            /* A key typed but not yet saved still counts: refusing the click
               then would send the reader back for a save they cannot see the
               need for. */
            disabled={unusable && !(setting && keys?.[setting]?.trim())}
            onclick={() => emit([...enabled, provider.id])}
          >
            {t('EnableSource')}
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>
