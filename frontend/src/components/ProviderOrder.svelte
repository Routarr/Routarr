<script lang="ts">
  import { ArrowDown, ArrowUp } from '../lib/icons';
  import { METADATA_FIELD_KEY, swapped } from '../api/format';
  import type { MetadataProvider } from '../api/types';
  import { t } from '../lib/i18n.svelte';
  import { handFocus } from '../lib/focus';
  import { FIELDS, SOURCE_EXTRA_SETTINGS, SOURCE_KEY_SETTING } from '../lib/settings';

  let {
    id,
    catalogue,
    value,
    onChange,
    keys,
    onKeyChange,
    storedKeys,
    removedKeys,
    onKeyRemove,
  }: {
    id: string;
    catalogue: MetadataProvider[];
    value: string;
    onChange: (value: string) => void;
    /**
     * The stored credentials, read only: an edit goes out through `onKeyChange`, since a write
     * into the record reaches the caller only when it passed a `$state` proxy.
     */
    keys?: Record<string, string>;
    /** Given with `keys` or not at all: without both, the field would take a key and drop it. */
    onKeyChange?: (setting: string, value: string) => void;
    /**
     * The credentials stored in the settings, which Remove clears, and those
     * a save is about to clear. A key the environment sets has no Remove: the
     * variable wins whatever is stored.
     */
    storedKeys?: ReadonlySet<string>;
    removedKeys?: ReadonlySet<string>;
    onKeyRemove?: (setting: string) => void;
  } = $props();

  const enabled = $derived(
    value
      .split(',')
      .map((entry) => entry.trim())
      .filter((entry) => catalogue.some((provider) => provider.id === entry)),
  );
  const disabled = $derived(catalogue.filter((provider) => !enabled.includes(provider.id)));

  const emit = (ids: string[]) => onChange(ids.join(','));

  const control = (providerId: string, part: 'up' | 'down' | 'toggle') =>
    `${id}-${providerId}-${part}`;

  function move(index: number, by: number) {
    const from = enabled[index];
    const next = swapped(enabled, index, by);
    if (!from || !next) return;
    emit(next);
    // A press that moves a row draws it again, and the pressed button with it,
    // or leaves that button disabled at the end of the list.
    const [ahead, back] = by < 0 ? (['up', 'down'] as const) : (['down', 'up'] as const);
    void handFocus(control(from, ahead), control(from, back));
  }

  function disable(providerId: string) {
    emit(enabled.filter((entry) => entry !== providerId));
    // Its Enable refused until a key is given, the key field is what is left.
    const setting = SOURCE_KEY_SETTING[providerId];
    void handFocus(control(providerId, 'toggle'), setting && `setting-${setting}`);
  }

  function enable(providerId: string) {
    emit([...enabled, providerId]);
    // The Arr's own row has no Disable, only its place in the order.
    void handFocus(
      control(providerId, 'toggle'),
      control(providerId, 'up'),
      control(providerId, 'down'),
    );
  }

  /** A source that is listed but cannot answer: it is waiting for a key. */
  const inert = (provider: MetadataProvider) => provider.needs_key && !provider.configured;
</script>

{#snippet credential(provider: MetadataProvider, setting: string)}
  <!-- The credential, in the row of the source it unlocks, drawn whether or not the source is on
       or a key stored: it is the only field for it, and the placeholder says which case. -->
  <label class="visually-hidden" for="setting-{setting}">
    {provider.display_name}
  </label>
  {@render masked(
    setting,
    provider.display_name,
    provider.configured
      ? t('SecretConfiguredPlaceholder')
      : provider.key_env
        ? t('ProviderKeyOrEnv', { variable: provider.key_env })
        : t('ProviderKeyPlaceholder'),
  )}
{/snippet}

{#snippet extras(providerId: string)}
  <!-- Under the key, captioned and explained: the source's name says what
       its key is, and nothing about what else it may ask for. -->
  {#each SOURCE_EXTRA_SETTINGS[providerId] ?? [] as setting (setting)}
    {@const field = FIELDS.find((entry) => entry.key === setting)}
    {#if field}
      <div class="source-extra">
        <label class="source-extra-label" for="setting-{setting}">{t(field.labelKey)}</label>
        {@render masked(
          setting,
          t(field.labelKey),
          storedKeys?.has(setting) ? t('SecretStoredPlaceholder') : undefined,
        )}
        <p id="setting-{setting}-help" class="source-detail">{t(field.helpKey)}</p>
      </div>
    {/if}
  {/each}
{/snippet}

{#snippet masked(setting: string, name: string, placeholder: string | undefined)}
  <div class="source-key">
    <input
      id="setting-{setting}"
      aria-describedby="setting-{setting}-help"
      class="form-input"
      type="password"
      autocomplete="off"
      placeholder={removedKeys?.has(setting) ? t('SecretRemovedOnSave') : placeholder}
      value={keys?.[setting] ?? ''}
      oninput={(event) => onKeyChange?.(setting, event.currentTarget.value)}
    />
    <!-- A blank field keeps the stored value, so without this a credential
         stays sealed behind a disabled source for good. -->
    {#if onKeyRemove && storedKeys?.has(setting) && !removedKeys?.has(setting)}
      <button
        type="button"
        class="btn btn-secondary"
        aria-label="{t('Remove')} – {name}"
        onclick={() => onKeyRemove(setting)}
      >
        {t('Remove')}
      </button>
    {/if}
  </div>
{/snippet}

{#snippet describe(provider: MetadataProvider)}
  {#if provider.needs_key && !provider.configured}
    <!-- The variable is named once, by the field below that accepts it, so
         a keyless source does not say it twice. -->
    {t('ProviderNeedsKey')}
  {:else}
    {provider.needs_key ? '' : `${t('ProviderNoKeyNeeded')} · `}{provider.fields
      .map((field) => t(METADATA_FIELD_KEY[field] ?? field))
      .join(t('ListSeparator'))}
  {/if}
  {provider.media_types.includes('movie') ? '' : ` · ${t('ProviderSeriesOnly')}`}
{/snippet}

<div {id}>
  <!-- Two named groups, so the active order and the rest are told apart. -->
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
              <!-- In words, not a tint alone: a source listed without its key answers nothing. -->
              {#if inactive}
                <span class="badge badge-warning">{t('ProviderInactive')}</span>
              {/if}
            </div>
            <div class="source-detail" id={setting ? `setting-${setting}-help` : undefined}>
              {@render describe(provider)}
            </div>
            {#if setting && keys && onKeyChange}
              {@render credential(provider, setting)}
              {@render extras(providerId)}
            {/if}
          </div>
          <div class="source-actions">
            <button
              id={control(providerId, 'up')}
              type="button"
              class="btn btn-secondary btn-sm"
              aria-label="{t('MoveUp')} – {provider.display_name}"
              disabled={index === 0}
              onclick={() => move(index, -1)}
            >
              <ArrowUp size={14} />
            </button>
            <button
              id={control(providerId, 'down')}
              type="button"
              class="btn btn-secondary btn-sm"
              aria-label="{t('MoveDown')} – {provider.display_name}"
              disabled={index === enabled.length - 1}
              onclick={() => move(index, 1)}
            >
              <ArrowDown size={14} />
            </button>
            <!-- The Arr's own metadata is always read: it costs no request
                 and every genre rule leans on it. -->
            {#if providerId !== 'arr'}
              <button
                id={control(providerId, 'toggle')}
                type="button"
                class="btn btn-secondary btn-sm"
                aria-label="{t('DisableSource')} – {provider.display_name}"
                onclick={() => disable(providerId)}
              >
                {t('DisableSource')}
              </button>
            {/if}
          </div>
        </div>
      {/if}
    {/each}
  </div>

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
            {@render extras(provider.id)}
          {/if}
        </div>
        <div class="source-actions">
          <button
            id={control(provider.id, 'toggle')}
            type="button"
            class="btn btn-secondary btn-sm"
            /* Refused where enabling changes nothing, unless a key is typed and not saved yet,
               which the save then stores. */
            disabled={unusable && !(setting && keys?.[setting]?.trim())}
            aria-label="{t('EnableSource')} – {provider.display_name}"
            onclick={() => enable(provider.id)}
          >
            {t('EnableSource')}
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>
