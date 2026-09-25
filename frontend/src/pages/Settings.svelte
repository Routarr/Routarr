<script lang="ts">
  import { untrack } from 'svelte';

  import { Download, KeyRound, Save, Trash2, Upload } from '../lib/icons';
  import { api, getApiKey, setApiKey } from '../api/client';
  import type { Category, MetadataProvider, Settings as SettingsMap } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { applyTheme, loadDictionary, t } from '../lib/i18n.svelte';
  import {
    FIELDS,
    SECTIONS,
    SOURCE_KEY_SETTING,
    type SectionId,
    type Field,
  } from '../lib/settings';
  import BackupCard from '../components/BackupCard.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import ProviderOrder from '../components/ProviderOrder.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import WarningBanner from '../components/WarningBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { downloadJson } from '../lib/download';

  const bundle = createAsync(async (signal) => {
    const [settings, categories, languages, metadata] = await Promise.all([
      api.getSettings(signal),
      api.getCategories(signal),
      api.getLanguages(signal),
      api.getMetadataProviders(signal),
    ]);
    return {
      settings,
      categories,
      languages: languages.languages,
      providers: metadata.providers,
    };
  });

  let draft = $state<SettingsMap>({});
  /**
   * What is actually stored, so the draft can be diffed against it.
   *
   * Saving covers every field of every section — the payload is built from all
   * of `FIELDS` — so the save bar has to say so, and has to say when there are
   * unsaved changes at all. Otherwise Routing is edited, Metadata is opened,
   * Save is pressed, and nothing on screen said what was written.
   */
  let saved = $state<SettingsMap>({});
  const outcome = createOutcome();
  let saving = $state(false);
  let key = $state(getApiKey());

  /**
   * Whether this browser has any use for a key, and what to say about it.
   *
   * The middleware reads one in `apikey`, `forms` and `oidc` and in no other
   * mode — `none` and `external` resolve an identity before they ever look at
   * the header, so a field there stores a string nothing will read. And in the
   * two session modes a key exists only if somebody set one, which is why the
   * server says so rather than the interface assuming it.
   */
  const auth = createAsync((signal) => api.authMode(signal));
  const keyCard = $derived.by(() => {
    const data = auth.data;
    if (!data) return null;
    if (data.mode === 'apikey') return { ...data, help: 'ApiKeyHelp' };
    if (data.mode === 'forms' || data.mode === 'oidc') {
      return { ...data, help: 'ApiKeyHelpSession' };
    }
    return null;
  });

  /**
   * The key a rotation just minted, shown until this screen is left.
   *
   * Held on screen and nowhere else: no route reads a key back, so this is the
   * one moment it can be copied. Storing it to show again later would make
   * every later visit a second chance to copy it, which is the property a
   * credential must not have.
   */
  let minted = $state<string | null>(null);

  async function rotateKey() {
    const existing = keyCard?.api_key_configured ?? false;
    if (existing && !(await askConfirmation(t('ConfirmRotateKey'), 'RegenerateKey'))) return;
    try {
      const { api_key } = await api.rotateApiKey();
      minted = api_key;
      // This browser is a client too, and in `apikey` mode it is holding the
      // key that just stopped working — the next request would 401 and drop the
      // screen behind the gate.
      setApiKey(api_key);
      key = api_key;
      outcome.clear();
      await auth.reload();
    } catch (cause) {
      outcome.fail(cause);
    }
  }

  async function removeKey() {
    if (!(await askConfirmation(t('ConfirmRemoveKey'), 'RemoveKey'))) return;
    try {
      await api.deleteApiKey();
      minted = null;
      setApiKey('');
      key = '';
      outcome.succeed(t('ApiKeyRemoved'));
      await auth.reload();
    } catch (cause) {
      outcome.fail(cause);
    }
  }

  // In the URL, so a section can be linked to and survives a reload. The hash
  // rather than a route: these are one page's sections, not twelve pages.
  const wanted = window.location.hash.replace('#', '');
  let section = $state<SectionId>(
    SECTIONS.some((entry) => entry.id === wanted) ? (wanted as SectionId) : 'general',
  );

  function openSection(id: SectionId) {
    section = id;
    // `replaceState`, not `pushState`: switching section is not navigation, and
    // stacking twenty history entries would make the back button useless.
    window.history.replaceState(null, '', `#${id}`);
  }

  // A hash that changes without a remount — a link to `#metadata` followed from
  // this very page — has to move the section too; read once at mount, the URL
  // and the screen disagree.
  $effect(() => {
    const sync = () => {
      const next = window.location.hash.replace('#', '');
      if (SECTIONS.some((entry) => entry.id === next)) section = next as SectionId;
    };
    window.addEventListener('hashchange', sync);
    return () => window.removeEventListener('hashchange', sync);
  });

  function seed(settings: SettingsMap) {
    // Backfill the keys the database has never been given a value for.
    const filled: SettingsMap = { ...settings };
    for (const field of FIELDS) {
      if (!filled[field.key]) filled[field.key] = field.fallback;
    }
    draft = filled;
    saved = { ...filled };
  }

  // A reload takes what is stored and puts every pending change back over it:
  // reseeded alone, a Retry would replace each unsaved change in every
  // section, and kept alone, a draft begun before the settings loaded would
  // save the fallbacks over the values it never read.
  $effect(() => {
    const data = bundle.data;
    if (!data) return;
    untrack(() => {
      const pending = Object.fromEntries(
        changed.map((field) => [field.key, draft[field.key] ?? field.fallback]),
      );
      seed(data.settings);
      draft = { ...draft, ...pending };
      // The screen wears the theme it has just read: an import replaces it
      // without a save.
      applyTheme(data.settings.ui_theme || 'dark');
    });
  });

  const changed = $derived(
    FIELDS.filter(
      (field) => (draft[field.key] ?? field.fallback) !== (saved[field.key] ?? field.fallback),
    ),
  );

  /** Whether a number sits outside the bounds the backend would refuse it for. */
  function outOfRange(field: Field): boolean {
    if (!field.range) return false;
    const value = Number(draft[field.key] ?? field.fallback);
    const [low, high] = field.range;
    return !Number.isInteger(value) || value < low || (high !== null && value > high);
  }
  const invalid = $derived(FIELDS.filter(outOfRange));

  // `SECTIONS[0]` is `| undefined` to the compiler even though the array is a
  // `const` with five entries, so the fallback is named rather than indexed.
  const GENERAL = SECTIONS[0];
  const active = $derived(SECTIONS.find((entry) => entry.id === section) ?? GENERAL);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);
  /**
   * What the metadata tab renders itself rather than through the field loop.
   *
   * The three credentials belong in the row of the source they unlock, and the
   * source list is a subject of its own rather than a field. Both are still in
   * `SECTIONS`, so the payload and the "covers every field exactly once" check
   * are untouched.
   */
  const SELF_RENDERED = new Set([...Object.values(SOURCE_KEY_SETTING), 'metadata_providers']);

  /**
   * The fields where blank means "leave the stored value alone".
   *
   * The credentials and nothing else: the backend never returns a sealed value,
   * so those fields are empty on every load. Every other setting is loaded with
   * what is stored, so an empty one is an empty one the reader chose.
   */
  const CREDENTIALS = new Set(Object.values(SOURCE_KEY_SETTING));

  const providers = $derived<MetadataProvider[]>(bundle.data?.providers ?? []);
  const languages = $derived(bundle.data?.languages ?? []);

  async function save(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    try {
      // A blank credential is left out rather than sent. The backend never
      // returns a sealed value, so the field is empty on every load; sending
      // that empty string writes it, and saving an unrelated setting would
      // delete the key. Leaving the field alone has to mean leaving the key
      // alone, which is what the `_configured` boolean is for.
      const payload = Object.fromEntries(
        FIELDS.map((field) => [field.key, draft[field.key] ?? field.fallback]).filter(
          ([key, value]) => !CREDENTIALS.has(key as string) || (value as string).trim() !== '',
        ),
      );
      await api.updateSettings(payload);
      applyTheme(payload.ui_theme);
      // The dictionary is served per language, so a language change needs a refetch.
      await loadDictionary();
      saved = payload;
      outcome.succeed(t('SettingsSaved'));
      // A metadata key added or cleared, or a source enabled: the warnings
      // about sources are computed from exactly these.
      invalidateStatus();
    } catch (err) {
      outcome.fail(err);
    } finally {
      saving = false;
    }
  }

  async function purge() {
    // Removes decisions, logs and jobs past their retention. Asked for first:
    // it is not undoable, and the button sits beside two that are harmless.
    if (!(await askConfirmation(t('ConfirmPurge'), 'PurgeNow'))) return;
    try {
      const report = await api.purge();
      outcome.succeed(
        t('PurgeResult', {
          decisions: report.decisions_removed,
          logs: report.logs_removed,
          jobs: report.jobs_removed,
        }),
      );
    } catch (err) {
      outcome.fail(err);
    }
  }

  async function exportConfig() {
    try {
      downloadJson(await api.exportConfig(), 'routarr-config.json');
      outcome.clear();
    } catch (err) {
      outcome.fail(err);
    }
  }

  async function importConfig(file: File) {
    try {
      const report = await api.importConfig(JSON.parse(await file.text()));
      // The import wrote every setting, so an edit begun before it goes now:
      // carried over the values read back, whenever that read succeeds, Save
      // would write it over the import.
      draft = { ...saved };
      // Before the summary, which is written in the language the import set.
      await loadDictionary();
      const restored =
        report.settings +
        report.categories +
        report.instances +
        report.root_folders +
        report.overrides;
      const waiting = report.needs_key.length
        ? ` ${t('ConfigImportNeedsKey', { names: report.needs_key.join(t('ListSeparator')) })}`
        : '';
      const summary =
        t('ConfigImportResult', {
          settings: report.settings,
          categories: report.categories,
          instances: report.instances,
          folders: report.root_folders,
          overrides: report.overrides,
        }) + waiting;
      // Never swallowed: a restore that quietly drops half a backup is worse
      // than one that fails.
      const partial = `${summary} ${t('ConfigImportSkipped', { count: report.skipped.length })}`;
      if (report.skipped.length > 0 && restored === 0) outcome.fail(partial, report.skipped);
      else if (report.skipped.length > 0) outcome.warn(partial, report.skipped);
      else if (waiting) outcome.warn(summary);
      else outcome.succeed(summary);
      // An import writes instances, categories, mappings and settings, four of
      // the seven things the warnings are computed from, so the shell's count
      // is stale the moment this returns.
      invalidateStatus();
      // The effect above reseeds the form from this read once it succeeds.
      await bundle.reload();
    } catch (err) {
      outcome.fail(err);
    }
  }

  function onTabKey(event: KeyboardEvent) {
    const index = SECTIONS.findIndex((entry) => entry.id === section);
    const next =
      event.key === 'ArrowRight'
        ? (index + 1) % SECTIONS.length
        : event.key === 'ArrowLeft'
          ? (index - 1 + SECTIONS.length) % SECTIONS.length
          : event.key === 'Home'
            ? 0
            : event.key === 'End'
              ? SECTIONS.length - 1
              : -1;
    // Read once and checked, rather than indexed twice: `next` is computed
    // from a key press and the two lookups could disagree if that ever grows a
    // branch.
    const target = SECTIONS[next];
    if (!target) return;
    event.preventDefault();
    openSection(target.id);
    document.getElementById(`tab-${target.id}`)?.focus();
  }
</script>

{#if bundle.loading}
  <Loading />
{:else}
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{t('Settings')}</h1>
        <p class="page-subtitle">{t('SettingsSubtitle')}</p>
      </div>
    </div>

    <!-- No Dismiss: Save waits for a read that succeeds, and only Retry gives
         it one. -->
    <ErrorBanner message={bundle.error} onRetry={() => void bundle.reload()} />
    <OutcomeBanner {outcome} />

    {#if draft.global_dry_run === 'false'}
      <WarningBanner message={t('LiveModeWarning')} />
    {/if}

    <!-- Only the combination writes unattended: either switch alone is inert. -->
    {#if draft.global_dry_run === 'false' && draft.auto_apply_enabled === 'true'}
      <WarningBanner message={t('AutoApplyWarning')} />
    {/if}

    <!-- One column for the strip and the panel together. The strip's rule ran
         the full width of the page while the card under it stopped at 720px and
         hugged the left edge — a full-width line over a narrow block, which is
         what made this screen read as broken rather than as deliberate. -->
    <div class="settings-column">
      <!-- A real tab strip: `aria-selected` says which one is open, and the
           arrow keys move between them, because a `role="tab"` that only
           responds to Tab is a lie told to a screen reader. -->
      <div class="tabs" role="tablist" aria-label={t('SettingsSections')}>
        {#each SECTIONS as entry (entry.id)}
          <button
            id="tab-{entry.id}"
            type="button"
            role="tab"
            class="tab {entry.id === section ? 'active' : ''}"
            aria-selected={entry.id === section}
            aria-controls="panel-{entry.id}"
            tabindex={entry.id === section ? 0 : -1}
            onclick={() => openSection(entry.id)}
            onkeydown={onTabKey}
          >
            {t(entry.labelKey)}
          </button>
        {/each}
      </div>

      <div id="panel-{section}" role="tabpanel" aria-labelledby="tab-{section}">
        {#if section === 'general' && keyCard}
          <div class="card">
            <div class="card-header">
              <h2 class="card-title">
                <KeyRound size={16} />
                {t('RoutarrApiKey')}
              </h2>
            </div>
            <p class="text-muted text-md mb-3">
              {t(keyCard.help)}
            </p>

            {#if minted}
              <!-- The one moment this value is readable. `mono` carries the
                   `direction: ltr` a hex string needs: it has no strong
                   character, so it would otherwise take the page's. -->
              <div class="mb-3">
                <WarningBanner message={t('ApiKeyMintedOnce')} />
                <code class="mono">{minted}</code>
              </div>
            {/if}

            {#if keyCard.api_key_configured}
              <div class="flex gap-2 mb-3">
                <input
                  type="password"
                  class="form-input"
                  aria-label={t('RoutarrApiKey')}
                  placeholder={t('ApiKeyPlaceholder')}
                  bind:value={key}
                />
                <button
                  type="button"
                  class="btn btn-secondary"
                  onclick={() => {
                    setApiKey(key);
                    outcome.succeed(t(key ? 'ApiKeyStored' : 'ApiKeyCleared'));
                  }}
                >
                  {t('SaveKey')}
                </button>
              </div>
            {/if}

            {#if keyCard.api_key_pinned}
              <!-- Rotating would mint a key the next restart replaces with the
                   variable's again, so the buttons are absent rather than
                   present and quietly futile. -->
              <p class="text-muted text-sm">{t('ApiKeyPinned')}</p>
            {:else}
              <div class="flex gap-2">
                <button type="button" class="btn btn-secondary" onclick={rotateKey}>
                  {t(keyCard.api_key_configured ? 'RegenerateKey' : 'CreateKey')}
                </button>
                {#if keyCard.api_key_configured && keyCard.mode !== 'apikey'}
                  <button type="button" class="btn btn-danger" onclick={removeKey}>
                    {t('RemoveKey')}
                  </button>
                {/if}
              </div>
            {/if}
          </div>
        {/if}

        {#if section === 'maintenance'}
          <BackupCard {outcome} />
        {/if}

        <form novalidate onsubmit={save}>
          <!-- The source list is a subject, not a field: rendered inside the
               loop it took a field's caption, so the card title, that caption
               and the "active / available" group labels competed at the same
               weight — and the sentence governing the whole thing sat under the
               rows it was meant to introduce. Its own card puts the sentence
               where it governs and drops the level that was doing nothing. -->
          {#if section === 'metadata'}
            {@const sources = FIELDS.find((field) => field.key === 'metadata_providers')}
            {#if sources}
              <div class="card">
                <div class="card-header">
                  <div>
                    <h2 class="card-title">{t(sources.labelKey)}</h2>
                    <p class="card-note">{t(sources.helpKey)}</p>
                  </div>
                </div>
                <ProviderOrder
                  id="setting-{sources.key}"
                  catalogue={providers}
                  value={draft[sources.key] ?? sources.fallback}
                  onChange={(value) => (draft[sources.key] = value)}
                  keys={draft}
                  onKeyChange={(setting, value) => (draft[setting] = value)}
                />
              </div>
            {/if}
          {/if}

          <div class="card">
            <div class="card-header">
              <h2 class="card-title">
                {t(section === 'metadata' ? 'SettingsMetadataCoverage' : active.labelKey)}
              </h2>
            </div>

            <!-- The three metadata credentials are rendered by the source list
               rather than here: a key is not a setting of the application, it
               is a property of the source it unlocks, and stated apart it left
               "enable TMDb" as a round trip past the whole list and back, with
               a save at each end. -->
            {#each FIELDS.filter((field) => (active.keys as readonly string[]).includes(field.key) && !SELF_RENDERED.has(field.key)) as field (field.key)}
              <div class="form-group">
                <!-- `for`/`id` rather than a bare sibling: without the pairing a
                   screen reader announces the control as unlabelled, and
                   clicking the caption does not focus the field. -->
                <label class="form-label" for="setting-{field.key}">{t(field.labelKey)}</label>

                {#if field.kind === 'bool'}
                  <select
                    id="setting-{field.key}"
                    aria-describedby="setting-{field.key}-help"
                    class="form-select"
                    value={draft[field.key] ?? field.fallback}
                    onchange={(event) => (draft[field.key] = event.currentTarget.value)}
                  >
                    <option value="true">{t('Enabled')}</option>
                    <option value="false">{t('Disabled')}</option>
                  </select>
                {:else if field.kind === 'language'}
                  <select
                    id="setting-{field.key}"
                    aria-describedby="setting-{field.key}-help"
                    class="form-select"
                    value={draft[field.key] ?? field.fallback}
                    onchange={(event) => (draft[field.key] = event.currentTarget.value)}
                  >
                    {#each languages as language (language.code)}
                      <!-- A partial translation is allowed; saying so is what
                         keeps the picker honest. Complete ones stay unadorned. -->
                      <option value={language.code}>
                        {language.completion >= 100
                          ? language.name
                          : `${language.name} (${language.completion}%)`}
                      </option>
                    {/each}
                  </select>
                {:else if field.kind === 'theme'}
                  <select
                    id="setting-{field.key}"
                    aria-describedby="setting-{field.key}-help"
                    class="form-select"
                    value={draft[field.key] ?? field.fallback}
                    onchange={(event) => (draft[field.key] = event.currentTarget.value)}
                  >
                    <option value="dark">{t('ThemeDark')}</option>
                    <option value="light">{t('ThemeLight')}</option>
                    <option value="auto">{t('ThemeAuto')}</option>
                  </select>
                {:else if field.kind === 'category'}
                  <select
                    id="setting-{field.key}"
                    aria-describedby="setting-{field.key}-help"
                    class="form-select"
                    value={draft[field.key] ?? field.fallback}
                    onchange={(event) => (draft[field.key] = event.currentTarget.value)}
                  >
                    {#each categories as category (category.id)}
                      <option value={category.name}>{category.name}</option>
                    {/each}
                  </select>
                {:else}
                  <input
                    id="setting-{field.key}"
                    aria-describedby="setting-{field.key}-help"
                    type={field.kind === 'number' ? 'number' : 'text'}
                    min={field.range?.[0]}
                    max={field.range?.[1] ?? undefined}
                    aria-invalid={outOfRange(field) ? 'true' : undefined}
                    class="form-input"
                    value={draft[field.key] ?? field.fallback}
                    oninput={(event) => (draft[field.key] = event.currentTarget.value)}
                  />
                {/if}

                <p id="setting-{field.key}-help" class="text-muted text-sm mt-1">
                  {t(field.helpKey)}
                </p>
              </div>
            {/each}

            <!-- Housekeeping on the left, the commit on the right —
               `justify-between` across four buttons would scatter them evenly
               and hide which one actually saves. -->
            <div class="flex justify-between items-center mt-4 gap-4">
              <div class="flex flex-wrap gap-2">
                {#if section === 'maintenance'}
                  <button
                    type="button"
                    class="btn btn-secondary"
                    /* The one thing an export cannot carry, said where the export
                     happens rather than only in the README. */
                    title={t('BackupTogetherHint')}
                    onclick={() => void exportConfig()}
                  >
                    <Download size={16} />
                    {t('ExportConfig')}
                  </button>
                  <label class="btn btn-secondary cursor-pointer">
                    <Upload size={16} />
                    {t('ImportConfig')}
                    <input
                      type="file"
                      accept="application/json"
                      hidden
                      onchange={(event) => {
                        const file = event.currentTarget.files?.[0];
                        if (file) void importConfig(file);
                        event.currentTarget.value = '';
                      }}
                    />
                  </label>
                  <button type="button" class="btn btn-secondary" onclick={purge}>
                    <Trash2 size={16} />
                    {t('PurgeNow')}
                  </button>
                {/if}
              </div>
            </div>

            {#if section === 'maintenance'}
              <p class="text-muted text-sm mt-3">
                {t('BackupTogetherHint')}
              </p>
            {/if}

            <!-- Sticky, and it says what it covers. Inside whichever section is
               open, a button that saves every field of every section has an
               invisible scope, and nothing signals that anything is pending. -->
            {#if changed.length > 0}
              <div class="save-bar" role="status">
                <div>
                  <strong>{t('UnsavedChanges', { count: changed.length })}</strong>
                  <div class="save-bar-scope">{t('SavesEverySection')}</div>
                </div>
                <div class="flex gap-2">
                  <button
                    type="button"
                    class="btn btn-secondary"
                    onclick={() => (draft = { ...saved })}
                  >
                    {t('DiscardChanges')}
                  </button>
                  <!-- Held while the settings could not be read: the draft
                       then holds values the server may no longer store. -->
                  <button
                    type="submit"
                    class="btn btn-primary"
                    disabled={saving || invalid.length > 0 || bundle.error !== null}
                  >
                    <Save size={16} />
                    {saving ? t('Saving') : t('Save')}
                  </button>
                </div>
              </div>
            {/if}
          </div>
        </form>
      </div>
    </div>
  </div>
{/if}
