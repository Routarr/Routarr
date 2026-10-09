<script lang="ts">
  import { untrack } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';

  import { AlertTriangle, Download, KeyRound, Save, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import { formatPercent } from '../api/format';
  import { withProof } from '../lib/proof.svelte';
  import type { Category, MetadataProvider, Settings as SettingsMap } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { handFocus } from '../lib/focus';
  import { applyTheme, i18n, loadDictionary, t } from '../lib/i18n.svelte';
  import {
    FIELDS,
    SECTIONS,
    SOURCE_KEY_SETTING,
    splitStored,
    type SectionId,
    type SettingsPage,
    type Field,
  } from '../lib/settings';
  import FileButton from '../components/FileButton.svelte';
  import BackupCard from '../components/BackupCard.svelte';
  import BackupEncryptionCard from '../components/BackupEncryptionCard.svelte';
  import AccountCard from '../components/AccountCard.svelte';
  import SessionsCard from '../components/SessionsCard.svelte';
  import NotificationTest from '../components/NotificationTest.svelte';
  import WebhookSigning from '../components/WebhookSigning.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import ProviderOrder from '../components/ProviderOrder.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import GuideStepBanner from '../components/GuideStepBanner.svelte';
  import WarningBanner from '../components/WarningBanner.svelte';
  import { ask, askConfirmation } from '../lib/confirm.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { onboarding, publishOnboarding } from '../lib/onboarding.svelte';
  import { href, navigate } from '../lib/router.svelte';
  import { holdUnsaved } from '../lib/unsaved.svelte';
  import { downloadJson } from '../lib/download';
  import { readJsonFile } from '../lib/upload';

  /**
   * The settings of one screen. The Settings screen and the metadata sources
   * screen each edit the sections naming them, and save those fields alone:
   * a save from one never writes over a draft left open on the other.
   */
  let { page }: { page: SettingsPage } = $props();

  // A screen's sections are fixed for its life, so neither is reactive.
  const sections = SECTIONS.filter((entry) => entry.page === page);
  const fields = FIELDS.filter((field) =>
    sections.some((entry) => (entry.keys as readonly string[]).includes(field.key)),
  );
  // One section needs no tab strip, and a strip of one would be read out as a
  // choice with nothing to choose.
  const tabbed = sections.length > 1;

  const bundle = createAsync(async (signal) => {
    const [answer, categories, languages, metadata] = await Promise.all([
      api.getSettings(signal),
      api.getCategories(signal),
      api.getLanguages(signal),
      api.getMetadataProviders(signal),
    ]);
    const { values, sealed } = splitStored(answer);
    // An unstored source list is the shipped default, which only the server
    // knows. Seeded from a copy kept here instead, the first save of any
    // setting would store that copy, since a save sends every field. A repeat
    // keeps its first place: the server refuses a list that repeats a source,
    // so a stored list holding one would hold back every save.
    const stored = values.metadata_providers;
    const listed = stored?.split(',').map((id) => id.trim()) ?? metadata.order;
    const settings: SettingsMap = {
      ...values,
      metadata_providers: [...new Set(listed.filter(Boolean))].join(','),
    };
    return {
      settings,
      sealed,
      categories,
      languages: languages.languages,
      providers: metadata.providers,
    };
  });

  let draft = $state<SettingsMap>({});
  /**
   * What is actually stored, so the draft can be diffed against it.
   *
   * Saving covers every field of every section of this screen (the payload is
   * built from all of `fields`), so the save bar has to say so, and has to say
   * when there are unsaved changes at all. Otherwise Routing is edited,
   * Maintenance is opened, Save is pressed, and nothing on screen says what was
   * written.
   */
  let saved = $state<SettingsMap>({});
  /** The sealed settings holding a value, which no field can show. */
  const stored = new SvelteSet<string>();
  /**
   * The sealed settings to remove at the next save. A blank credential field
   * means "leave it alone", so removing one is a state of its own.
   */
  const removing = new SvelteSet<string>();
  const outcome = createOutcome();
  let saving = $state(false);

  /**
   * Whether this browser has any use for a key, and what to say about it.
   *
   * The middleware reads one in `apikey`, `forms` and `oidc` and in no other
   * mode: `none` and `external` resolve an identity before they ever look at
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
      const rotated = await withProof((proof) => api.rotateApiKey(proof));
      if (!rotated) return;
      minted = rotated.api_key;
      outcome.clear();
      await auth.reload();
    } catch (cause) {
      outcome.fail(cause);
    }
  }

  async function removeKey() {
    if (!(await askConfirmation(t('ConfirmRemoveKey'), 'RemoveKey'))) return;
    try {
      if ((await withProof((proof) => api.deleteApiKey(proof))) === null) return;
      minted = null;
      outcome.succeed(t('ApiKeyRemoved'));
      await auth.reload();
    } catch (cause) {
      outcome.fail(cause);
    }
  }

  // In the URL, so a section can be linked to and survives a reload. The hash
  // rather than a route: these are one page's sections, not pages of their own.
  const wanted = window.location.hash.replace('#', '');
  // `sections[0]` is `| undefined` to the compiler, though every page names at
  // least one section, so the fallback is the first of the whole table.
  const first = sections[0] ?? SECTIONS[0];
  let section = $state<SectionId>(
    tabbed && sections.some((entry) => entry.id === wanted) ? (wanted as SectionId) : first.id,
  );

  function openSection(id: SectionId) {
    section = id;
    // `replaceState`, not `pushState`: switching section is not navigation, and
    // stacking twenty history entries would make the back button useless. The
    // path is written out: a bare `#id` resolves against the `<base href>`,
    // which is the mount point and not this page.
    const { pathname, search } = window.location;
    window.history.replaceState(null, '', `${pathname}${search}#${id}`);
  }

  // A hash that changes without a remount, as a link to `#guardrails` followed
  // from this very page, has to move the section too. Read once at mount, the
  // URL and the screen disagree.
  $effect(() => {
    if (!tabbed) return;
    const sync = () => {
      const next = window.location.hash.replace('#', '');
      if (sections.some((entry) => entry.id === next)) section = next as SectionId;
    };
    window.addEventListener('hashchange', sync);
    return () => window.removeEventListener('hashchange', sync);
  });

  function seed(settings: SettingsMap) {
    // Backfill the keys the database has never been given a value for.
    const filled: SettingsMap = { ...settings };
    for (const field of fields) {
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
      stored.clear();
      for (const setting of data.sealed) stored.add(setting);
      // The screen wears the theme it has just read: an import replaces it
      // without a save.
      applyTheme(data.settings.ui_theme || 'dark');
    });
  });

  const changed = $derived(
    fields.filter(
      (field) =>
        removing.has(field.key) ||
        (draft[field.key] ?? field.fallback) !== (saved[field.key] ?? field.fallback),
    ),
  );

  holdUnsaved(
    () => changed.length > 0,
    () => t('ConfirmLeaveUnsaved', { count: changed.length }),
  );

  /** Whether a number sits outside the bounds the backend would refuse it for. */
  function outOfRange(field: Field): boolean {
    if (!field.range) return false;
    const value = Number(draft[field.key] ?? field.fallback);
    const [low, high] = field.range;
    return !Number.isInteger(value) || value < low || value > high;
  }
  const invalid = $derived(fields.filter(outOfRange));
  // From another tab, the field that holds Save is out of sight.
  const flagged = $derived(
    sections
      .filter((entry) =>
        invalid.some((field) => (entry.keys as readonly string[]).includes(field.key)),
      )
      .map((entry) => entry.id),
  );

  function bounds([low, high]: readonly [number, number]): string {
    return t('RangeBetween', { min: low, max: high });
  }

  const active = $derived(sections.find((entry) => entry.id === section) ?? first);
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
  const CREDENTIALS = new Set(
    FIELDS.filter((field) => field.kind === 'secret').map((field) => field.key),
  );

  function remove(field: Field) {
    removing.add(field.key);
    draft[field.key] = '';
    // The button goes with the value, and the field says what Save will do.
    void handFocus(`setting-${field.key}`);
  }

  const providers = $derived<MetadataProvider[]>(bundle.data?.providers ?? []);
  const languages = $derived(bundle.data?.languages ?? []);

  // Save and Discard live in the save bar, which goes once nothing is pending,
  // and the focus with it: the open section's panel takes it back.
  const keepFocus = () => void handFocus(`panel-${section}`);

  function discard() {
    draft = { ...saved };
    removing.clear();
    keepFocus();
  }

  // Never disabled while it runs: a focused button that turns disabled drops
  // the focus to the page, so a second press is ignored instead.
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (saving) return;
    saving = true;
    try {
      // A blank credential is left out rather than sent. The backend never
      // returns a sealed value, so the field is empty on every load, and
      // sending that emptiness would delete the credential on every unrelated
      // save. One the reader asked to remove is sent empty, which is how the
      // backend removes it.
      const payload: SettingsMap = Object.fromEntries(
        fields.flatMap((field) => {
          const value = draft[field.key] ?? field.fallback;
          if (!CREDENTIALS.has(field.key) || value.trim() !== '') return [[field.key, value]];
          return removing.has(field.key) ? [[field.key, '']] : [];
        }),
      );
      await api.updateSettings(payload);
      for (const [setting, value] of Object.entries(payload)) {
        if (!CREDENTIALS.has(setting)) continue;
        if (value.trim() === '') stored.delete(setting);
        else stored.add(setting);
      }
      removing.clear();
      if (payload.ui_theme) applyTheme(payload.ui_theme);
      // The dictionary is served per language, so a language change needs a refetch.
      await loadDictionary();
      saved = { ...saved, ...payload };
      outcome.succeed(t('SettingsSaved'));
      // A metadata key added or cleared, or a source enabled: the warnings
      // about sources are computed from exactly these.
      invalidateStatus();
      keepFocus();
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
      // Pending decisions past retention go too, and the Simulation count in
      // the navigation would keep them until its next idle poll.
      invalidateStatus();
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

  // Offered in every state, so the one place the guide is said to live never
  // turns up empty. A guide already showing needs only the way back to it.
  async function restartGuide() {
    try {
      if (onboarding.current?.state !== 'pending') {
        publishOnboarding(await api.setOnboarding('pending'));
      }
      navigate('/');
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

  /** Whether a bundle read from a file carries rules, which then need a question. */
  function carriesRules(bundle: unknown): boolean {
    const rules = (bundle as { rules?: unknown } | null)?.rules;
    return Array.isArray(rules) && rules.length > 0;
  }

  async function importConfig(file: File) {
    try {
      const parsed = await readJsonFile(file);
      // The question the Rules screen asks of a rule file, with its three
      // outcomes: Cancel imports nothing.
      let replaceRules = false;
      if (carriesRules(parsed)) {
        const answer = await ask(t('ImportReplaceQuestion'), [
          { label: 'ImportAppend', value: 'append' },
          { label: 'ImportReplace', value: 'replace', danger: true },
        ]);
        if (answer === null) return;
        replaceRules = answer === 'replace';
      }
      const report = await api.importConfig(parsed, replaceRules);
      // The import wrote every setting, so an edit begun before it goes now:
      // carried over the values read back, whenever that read succeeds, Save
      // would write it over the import.
      draft = { ...saved };
      removing.clear();
      // Before the summary, which is written in the language the import set.
      await loadDictionary();
      const restored =
        report.settings +
        report.categories +
        report.instances +
        report.root_folders +
        report.rules +
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
          rules: report.rules,
          overrides: report.overrides,
        }) + waiting;
      // Never swallowed: a restore that quietly drops half a backup is worse
      // than one that fails.
      const partial = `${summary} ${t('ConfigImportSkipped', { count: report.skipped.length })}`;
      if (report.skipped.length > 0 && restored === 0) outcome.fail(partial, report.skipped);
      else if (report.skipped.length > 0)
        outcome.warn(partial, [...report.skipped, ...report.adjusted]);
      else if (waiting || report.adjusted.length > 0) outcome.warn(summary, report.adjusted);
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
    const index = sections.findIndex((entry) => entry.id === section);
    // The next tab is drawn where the text goes: on the left, right to left.
    const [forward, back] =
      i18n.direction === 'rtl' ? ['ArrowLeft', 'ArrowRight'] : ['ArrowRight', 'ArrowLeft'];
    const next =
      event.key === forward
        ? (index + 1) % sections.length
        : event.key === back
          ? (index - 1 + sections.length) % sections.length
          : event.key === 'Home'
            ? 0
            : event.key === 'End'
              ? sections.length - 1
              : -1;
    // Read once and checked, rather than indexed twice: `next` is computed
    // from a key press and the two lookups could disagree if that ever grows a
    // branch.
    const target = sections[next];
    if (!target) return;
    event.preventDefault();
    openSection(target.id);
    document.getElementById(`tab-${target.id}`)?.focus();
  }
</script>

<!-- Above the placeholder, so an action's outcome is said in the region that
     was there before the reload it starts. -->
<OutcomeBanner {outcome} />
<!-- On the first read only: over a reload it would take the form, the pressed
     button and the focus with it. -->
{#if bundle.loading && bundle.data === null}
  <div class="card"><Loading /></div>
{:else}
  <!-- No Dismiss: Save waits for a read that succeeds, and only Retry gives
         it one. -->
  <ErrorBanner message={bundle.error} onRetry={() => void bundle.reload()} />
  <!-- The guide's two optional steps are done on these screens. -->
  {#if section === 'metadata'}
    <GuideStepBanner step="metadata" />
  {:else if section === 'guardrails'}
    <GuideStepBanner step="live" />
  {/if}

  <!-- Said where the two switches are edited, and nowhere else. -->
  {#if page === 'settings' && draft.global_dry_run === 'false'}
    <WarningBanner message={t('LiveModeWarning')} />
  {/if}

  <!-- Only the combination writes unattended: either switch alone is inert. -->
  {#if page === 'settings' && draft.global_dry_run === 'false' && draft.auto_apply_enabled === 'true'}
    <WarningBanner message={t('AutoApplyWarning')} />
  {/if}

  <!-- One column for the strip and the panel together. A strip whose rule
         runs the full width of the page, over a narrower card hugging the left
         edge, reads as broken rather than as deliberate. -->
  <div class="settings-column">
    <!-- A real tab strip: `aria-selected` says which one is open, and the
           arrow keys move between them, because a `role="tab"` that only
           responds to Tab is a lie told to a screen reader. -->
    {#if tabbed}
      <div class="tabs" role="tablist" aria-label={t('SettingsSections')}>
        {#each sections as entry (entry.id)}
          <button
            id="tab-{entry.id}"
            type="button"
            role="tab"
            class="tab {entry.id === section ? 'active' : ''}"
            aria-selected={entry.id === section}
            aria-controls="panel-{entry.id}"
            aria-label={flagged.includes(entry.id)
              ? t('SectionHasInvalid', { section: t(entry.labelKey) })
              : undefined}
            tabindex={entry.id === section ? 0 : -1}
            onclick={() => openSection(entry.id)}
            onkeydown={onTabKey}
          >
            {t(entry.labelKey)}
            {#if flagged.includes(entry.id)}
              <AlertTriangle size={14} class="tab-flag" aria-hidden="true" />
            {/if}
          </button>
        {/each}
      </div>
    {/if}

    <div
      id="panel-{section}"
      role={tabbed ? 'tabpanel' : undefined}
      aria-labelledby={tabbed ? `tab-${section}` : undefined}
      tabindex="-1"
    >
      {#if section === 'security' && keyCard}
        <div class="card">
          <div class="card-header">
            <div>
              <h2 class="card-title flex items-center gap-2">
                <KeyRound size={18} aria-hidden="true" />
                {t('RoutarrApiKey')}
              </h2>
              <p class="card-note">{t(keyCard.help)}</p>
            </div>
          </div>

          {#if minted}
            <!-- The one moment this value is readable. `mono` carries the
                   `direction: ltr` a hex string needs: it has no strong
                   character, so it would otherwise take the page's. -->
            <div class="mb-3">
              <WarningBanner message={t('ApiKeyMintedOnce')} />
              <code class="mono secret-once">{minted}</code>
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

      {#if section === 'security' && auth.data?.mode === 'forms'}
        <AccountCard {outcome} />
      {/if}

      <!-- Where a browser signs in: an open instance, or one behind a proxy
           that authenticates for it, holds no session. -->
      {#if section === 'security' && (auth.data?.mode === 'forms' || auth.data?.mode === 'oidc' || auth.data?.mode === 'apikey')}
        <SessionsCard {outcome} />
      {/if}

      {#if section === 'general'}
        <div class="card">
          <div class="card-header">
            <div>
              <h2 class="card-title">{t('GuideTitle')}</h2>
              <p class="card-note">{t('GuideRestartText')}</p>
            </div>
            <button type="button" class="btn btn-secondary" onclick={() => void restartGuide()}>
              {t('GuideRestart')}
            </button>
          </div>
        </div>
      {/if}

      {#if section === 'maintenance'}
        <BackupEncryptionCard
          {outcome}
          configured={bundle.data?.sealed.includes('backup_passphrase') ?? false}
        />
        <BackupCard {outcome} />
      {/if}

      <form novalidate onsubmit={save}>
        <!-- The source list is a subject, not a field. Rendered inside the
               loop it would take a field's caption, and the card title, that
               caption and the "active / available" group labels would compete
               at the same weight, with the sentence governing the whole thing
               under the rows it introduces. Its own card puts the sentence
               where it governs, with no level between. -->
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
                storedKeys={stored}
                removedKeys={removing}
                onKeyRemove={(setting) => {
                  const field = FIELDS.find((entry) => entry.key === setting);
                  if (field) remove(field);
                }}
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
               is a property of the source it unlocks, and stated apart it would
               make "enable TMDB" a round trip past the whole list and back,
               with a save at each end. -->
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
                  <!-- Keys of their own: a setting's state agrees otherwise than
                         the rule and the instance the badges name, in French and
                         Italian among others. -->
                  <option value="true">{t('SettingOn')}</option>
                  <option value="false">{t('SettingOff')}</option>
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
                    <!-- A partial translation is allowed, and saying so is what
                         keeps the picker honest. Complete ones stay unadorned. -->
                    <option value={language.code}>
                      {language.completion >= 100
                        ? language.name
                        : `${language.name} (${formatPercent(language.completion / 100, i18n.language)})`}
                    </option>
                  {/each}
                </select>
              {:else if field.kind === 'choice'}
                <select
                  id="setting-{field.key}"
                  aria-describedby="setting-{field.key}-help"
                  class="form-select"
                  value={draft[field.key] ?? field.fallback}
                  onchange={(event) => (draft[field.key] = event.currentTarget.value)}
                >
                  {#each field.choices ?? [] as choice (choice.value)}
                    <option value={choice.value}>
                      {'name' in choice ? choice.name : t(choice.labelKey)}
                    </option>
                  {/each}
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
              {:else if field.kind === 'secret'}
                <!-- Never returned by the server, so empty on every load: the
                       placeholder says whether a value is stored, and only
                       Remove clears one, since a blank field leaves it alone. -->
                <div class="flex gap-2">
                  <input
                    id="setting-{field.key}"
                    aria-describedby="setting-{field.key}-help"
                    type="password"
                    autocomplete="off"
                    class="form-input"
                    placeholder={removing.has(field.key)
                      ? t('SecretRemovedOnSave')
                      : stored.has(field.key)
                        ? t('SecretStoredPlaceholder')
                        : undefined}
                    value={draft[field.key] ?? ''}
                    oninput={(event) => (draft[field.key] = event.currentTarget.value)}
                  />
                  {#if stored.has(field.key) && !removing.has(field.key)}
                    <button
                      type="button"
                      class="btn btn-secondary"
                      aria-label="{t('Remove')} – {t(field.labelKey)}"
                      onclick={() => remove(field)}
                    >
                      {t('Remove')}
                    </button>
                  {/if}
                </div>
              {:else}
                {@const outside = outOfRange(field)}
                <input
                  id="setting-{field.key}"
                  aria-describedby="setting-{field.key}-help{outside
                    ? ` setting-${field.key}-range`
                    : ''}"
                  type={field.kind === 'number' ? 'number' : 'text'}
                  min={field.range?.[0]}
                  max={field.range?.[1]}
                  aria-invalid={outside ? 'true' : undefined}
                  class="form-input"
                  value={draft[field.key] ?? field.fallback}
                  oninput={(event) => (draft[field.key] = event.currentTarget.value)}
                />
                {#if outside && field.range}
                  <p id="setting-{field.key}-range" class="field-error">{bounds(field.range)}</p>
                {/if}
              {/if}

              <p id="setting-{field.key}-help" class="text-muted text-sm mt-1">
                {t(field.helpKey)}
              </p>
            </div>
          {/each}

          {#if section === 'maintenance'}
            <div class="flex flex-wrap gap-2 mt-4">
              <button type="button" class="btn btn-secondary" onclick={() => void exportConfig()}>
                <Download size={16} />
                {t('ExportConfig')}
              </button>
              <FileButton
                label={t('ImportConfig')}
                accept="application/json"
                onFile={(file) => void importConfig(file)}
              />
              <button type="button" class="btn btn-secondary" onclick={purge}>
                <Trash2 size={16} />
                {t('PurgeNow')}
              </button>
            </div>
            <p class="text-muted text-sm mt-3">
              {t('BackupCopyHint')}
            </p>
          {/if}

          <!-- Sticky, and it says what it covers. Inside whichever section is
               open, a button that saves every field of every section of the
               screen has an invisible scope, and nothing signals that anything
               is pending. -->
          {#if section === 'security'}
            <p class="text-muted text-sm mt-3">
              <a class="text-link" href={href('/security-log')}>{t('OpenSecurityLog')}</a>
            </p>
          {/if}

          {#if changed.length > 0}
            <div class="save-bar" role="status">
              <div>
                <strong>{t('UnsavedChanges', { count: changed.length })}</strong>
                {#if tabbed}
                  <div class="save-bar-scope">{t('SavesEverySection')}</div>
                {/if}
                {#if invalid.length > 0}
                  <div class="field-error" id="settings-save-held">{t('SaveHeldInvalid')}</div>
                {/if}
              </div>
              <div class="flex gap-2">
                <button type="button" class="btn btn-secondary" onclick={discard}>
                  {t('DiscardChanges')}
                </button>
                <!-- Held while the settings could not be read: the draft
                       then holds values that may not match what the server
                       stores. -->
                <button
                  type="submit"
                  class="btn btn-primary"
                  aria-describedby={invalid.length > 0 ? 'settings-save-held' : undefined}
                  disabled={invalid.length > 0 || bundle.error !== null}
                >
                  <Save size={16} />
                  {saving ? t('Saving') : t('Save')}
                </button>
              </div>
            </div>
          {/if}
        </div>
      </form>

      {#if section === 'notifications'}
        <NotificationTest {outcome} />
        <WebhookSigning {outcome} />
      {/if}
    </div>
  </div>
{/if}
