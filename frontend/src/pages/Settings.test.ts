import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { statusRevision } from '../lib/status.svelte';
import { ApiError, api } from '../api/client';
import type { AuthMode } from '../api/types';
import { FIELDS, SOURCE_KEY_SETTING } from '../lib/settings';
import Settings from './Settings.svelte';
import { answerConfirmation } from '../test/confirm';

/**
 * Settings is where unattended writing gets armed, so the warning that says so
 * is a guardrail rather than decoration: neither switch is dangerous alone, and
 * the banner must appear for the combination and only for it.
 */

const STRINGS = {
  Settings: 'Settings',
  LiveModeWarning: 'Live mode is on',
  AutoApplyWarning: 'Automatic application is armed',
  SettingsSaved: 'Saved',
  SettingsSections: 'Sections',
  SettingsTabRouting: 'Routing',
  SettingBatchLimit: 'Batch limit',
  SettingsTabAutomation: 'Automation',
  Metadata: 'Metadata',
  SettingsTabGeneral: 'General',
  SettingsTabMaintenance: 'Maintenance',
  UnsavedChanges: 'Unsaved changes: {count}',
  SavesEverySection: 'Every section is saved together',
  DiscardChanges: 'Discard',
  Save: 'Save',
  Enabled: 'Enabled',
  Disabled: 'Disabled',
  MoveUp: 'Move up',
  MoveDown: 'Move down',
  DisableSource: 'Disable',
  EnableSource: 'Enable',
  ProviderInactive: 'inactive',
  ProviderNeedsKey: 'needs an API key',
  SourcesActive: 'Active, in priority order',
  SourcesInactive: 'Inactive',
  ProviderKeyOrEnv: 'API key, or the {variable} variable',
  ProviderKeyPlaceholder: 'API key',
  RoutarrApiKey: 'Routarr API key',
  ApiKeyHelp: 'Generated at first start',
  ApiKeyHelpSession: 'For clients that cannot hold a session',
  ApiKeyPinned: 'The environment sets it',
  ApiKeyMintedOnce: 'Copy it now',
  RegenerateKey: 'Regenerate',
  CreateKey: 'Create a key',
  RemoveKey: 'Remove the key',
  ConfirmRotateKey: 'Regenerate the key?',
  ConfirmRemoveKey: 'Remove the key?',
  ApiKeyRemoved: 'API key removed.',
  PurgeNow: 'Purge now',
  ConfirmPurge: 'Remove everything past its retention?',
  PurgeResult: '{decisions} decisions, {logs} logs, {jobs} jobs removed',
  Backups: 'Backups',
  NoBackupsYet: 'No backup yet',
  BackupNow: 'Back up now',
  SettingGlobalDryRun: 'Global dry-run',
  SettingAutoApplyEnabled: 'Apply automatically',
  SecretConfiguredPlaceholder: 'A key is stored – type to replace it',
  ConfigImportResult: 'Restored: {settings} settings.',
  ConfigImportSkipped: 'Not restored: {count}',
  ConfigImportNeedsKey: 'Instances waiting for their API key: {names}',
  ListSeparator: ', ',
};

const APIKEY_MODE: AuthMode = {
  mode: 'apikey',
  api_key_configured: true,
  api_key_pinned: false,
};

type ProviderOverride = { configured?: boolean; order?: string[] };

function mount(
  settings: Record<string, string>,
  auth: AuthMode = APIKEY_MODE,
  provider: ProviderOverride = {},
) {
  vi.spyOn(api, 'authMode').mockResolvedValue(auth);
  vi.spyOn(api, 'getSettings').mockResolvedValue(settings);
  vi.spyOn(api, 'getCategories').mockResolvedValue([]);
  vi.spyOn(api, 'getLanguages').mockResolvedValue({
    default: 'en',
    languages: [{ code: 'en', name: 'English', completion: 100, direction: 'ltr' }],
  });
  // The backups card loads on mount; unmocked it would reach the network.
  vi.spyOn(api, 'listBackups').mockResolvedValue({ backups: [], retention_count: 7 });
  vi.spyOn(api, 'getMetadataProviders').mockResolvedValue({
    providers: [
      {
        id: 'arr',
        display_name: 'Radarr / Sonarr',
        fetched: false,
        needs_key: false,
        key_env: null,
        configured: true,
        fields: ['genres'],
      },
      {
        id: 'tmdb',
        display_name: 'TMDb',
        fetched: true,
        needs_key: true,
        key_env: 'TMDB_API_KEY',
        configured: provider.configured ?? false,
        fields: ['genres', 'keywords'],
      },
    ],
    order: provider.order ?? ['arr'],
  });
  return renderWithI18n(Settings, { strings: STRINGS });
}

/** Open a settings section, the way a user reaches it. */
const openSection = async (name: string) =>
  fireEvent.click(await screen.findByRole('tab', { name }));

/** Click Save and hand back what went to the server. */
async function save(): Promise<Record<string, string>> {
  const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(undefined as never);
  vi.spyOn(api, 'getLocalization').mockResolvedValue({
    language: 'en',
    direction: 'ltr',
    strings: STRINGS,
  });
  await fireEvent.click(await screen.findByRole('button', { name: 'Save' }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  return nthCall(update)[0] as Record<string, string>;
}

afterEach(() => {
  vi.restoreAllMocks();
  window.location.hash = '';
});

describe('the unattended-writing warning', () => {
  it('stays quiet while dry run is on', async () => {
    mount({ global_dry_run: 'true', auto_apply_enabled: 'true' });

    await screen.findByRole('heading', { name: 'Settings', level: 1 });
    expect(screen.queryByText('Automatic application is armed')).toBeNull();
  });

  it('stays quiet while automatic application is off', async () => {
    mount({ global_dry_run: 'false', auto_apply_enabled: 'false' });

    await screen.findByText('Live mode is on');
    expect(screen.queryByText('Automatic application is armed')).toBeNull();
  });

  /** Only the combination writes without anyone asking. */
  it('warns for the combination', async () => {
    mount({ global_dry_run: 'false', auto_apply_enabled: 'true' });

    expect(await screen.findByText('Automatic application is armed')).toBeTruthy();
    expect(screen.getByText('Live mode is on')).toBeTruthy();
  });
});

describe('the save bar', () => {
  it('is absent until something is edited', async () => {
    mount({ global_dry_run: 'true' });

    await screen.findByRole('heading', { name: 'Settings', level: 1 });
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
  });

  /**
   * Saving covers every field of every section, so the button has to say so.
   * Sitting inside whichever section is open, it says neither that nor that
   * there are unsaved changes at all.
   */
  it('appears with a count once a field changes, and says what it covers', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');

    expect(await screen.findByText('Unsaved changes: 1')).toBeTruthy();
    expect(screen.getByText('Every section is saved together')).toBeTruthy();
  });

  it('sends every field, not only the section on screen', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    const payload = await save();
    // Every field except the three credentials, which are blank here and are
    // left out rather than sent — see the test below for why that matters.
    const credentials = new Set(Object.values(SOURCE_KEY_SETTING));
    expect(Object.keys(payload).sort()).toEqual(
      FIELDS.map((f) => f.key)
        .filter((key) => !credentials.has(key))
        .sort(),
    );
  });

  /// A metadata key saved or cleared is what the source warnings are computed
  /// from, so saving has to tell the shell to read them again.
  it('tells the shell its counters are out of date', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');

    const before = statusRevision();
    await save();

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  it('puts the draft back when the change is discarded', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    await screen.findByText('Unsaved changes: 1');
    await fireEvent.click(screen.getByRole('button', { name: 'Discard' }));

    await waitFor(() => expect(screen.queryByText('Unsaved changes: 1')).toBeNull());
  });
});

describe('the metadata sources', () => {
  /**
   * A source that needs a key it has not got is shown, but never as if it
   * worked: putting one at position 1 looks like a configuration and behaves
   * like an absence.
   */
  /**
   * The credential lives in the row of the source it unlocks.
   *
   * Stated three blocks further down, turning on TMDb meant noticing a greyed
   * button that said nothing, scrolling past the whole list, pasting, saving,
   * scrolling back and saving again — two saves for one intention. A key is
   * not a setting of the application; it is a property of a source.
   */
  it('offers the credential in the row of the source that needs it', async () => {
    mount({ metadata_providers: 'arr' });
    await openSection('Metadata');

    const field = await screen.findByLabelText('TMDb');
    expect(field.getAttribute('id')).toBe('setting-tmdb_api_key');
    // The variable is named once, by the field that accepts it.
    expect(field.getAttribute('placeholder')).toBe('API key, or the TMDB_API_KEY variable');
  });

  it('refuses to add a source that cannot answer, until a key is given', async () => {
    mount({ metadata_providers: 'arr' });
    await openSection('Metadata');

    const enable = (
      await screen.findAllByRole('button', { name: 'Enable' })
    )[0] as HTMLButtonElement;
    expect(enable.disabled).toBe(true);

    // A key typed but not yet saved counts: refusing the click then would send
    // the reader back for a save they cannot see the need for.
    await userEvent.type(screen.getByLabelText('TMDb'), 'a-key');
    expect(enable.disabled).toBe(false);
  });

  /**
   * The row is the only place this field exists, so it cannot disappear once
   * the source works: a key that leaked has to be replaceable, and one held
   * only in the environment has to be enterable. Hidden as soon as
   * `configured` turned true, rotating a credential meant editing the
   * database by hand.
   */
  it('still offers the field once a key is stored and the source is on', async () => {
    mount({ metadata_providers: 'arr,tmdb' }, APIKEY_MODE, {
      configured: true,
      order: ['arr', 'tmdb'],
    });
    await openSection('Metadata');

    const field = await screen.findByLabelText('TMDb');
    expect(field.getAttribute('id')).toBe('setting-tmdb_api_key');
    // The placeholder is what says which of the two situations the reader is
    // in, since the backend never sends a sealed value back.
    expect(field.getAttribute('placeholder')).toBe('A key is stored – type to replace it');
  });

  /**
   * The field is empty on every load — the backend never returns a sealed
   * value — so sending it would write that emptiness. Saving an unrelated
   * setting deleted all three credentials, and nothing on screen said so:
   * the sources simply stopped answering on the next pass.
   */
  it('does not delete a stored key when an unrelated setting is saved', async () => {
    mount({ global_dry_run: 'true', metadata_providers: 'arr,tmdb' }, APIKEY_MODE, {
      configured: true,
      order: ['arr', 'tmdb'],
    });
    await openSection('Routing');

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    const payload = await save();
    expect(payload).not.toHaveProperty('tmdb_api_key');
    // What was actually edited still goes.
    expect(payload.global_dry_run).toBe('false');
  });

  it('sends a credential the reader did type', async () => {
    mount({ metadata_providers: 'arr' });
    await openSection('Metadata');

    await userEvent.type(await screen.findByLabelText('TMDb'), 'a-key');
    expect((await save()).tmdb_api_key).toBe('a-key');
  });

  /**
   * A setting the form shows is a setting the form sends.
   *
   * "Blank means leave the stored value alone" exists for one reason: the
   * backend never returns a sealed credential, so that field is empty on every
   * load and sending the emptiness would delete the key. The rule holds for a
   * secret and for nothing else — applied to the source list it dropped the one
   * setting whose empty value means something, showed "Settings saved", and
   * left the sources answering exactly as before.
   *
   * An empty list is refused by the backend, which is the point: the operator
   * is told the list is invalid instead of being told it was saved.
   */
  it('sends an emptied source list rather than dropping it', async () => {
    // A stored order without `arr`, so its last source carries the button the
    // Arr's own row deliberately does not.
    mount({ metadata_providers: 'tmdb' }, APIKEY_MODE, { order: ['tmdb'] });
    await openSection('Metadata');

    await fireEvent.click(await screen.findByRole('button', { name: 'Disable' }));
    const payload = await save();
    expect(payload).toHaveProperty('metadata_providers');
    expect(payload.metadata_providers).toBe('');
  });

  it('separates what is active from what is switched off', async () => {
    mount({ metadata_providers: 'arr' });
    await openSection('Metadata');

    expect(await screen.findByText('Active, in priority order')).toBeTruthy();
    // "Available" named exactly the sources that are not: a keyless one can
    // answer nothing at all.
    expect(screen.getByText('Inactive')).toBeTruthy();
  });

  it('cannot move the only active source', async () => {
    mount({ metadata_providers: 'arr' });
    await openSection('Metadata');

    const up = await screen.findByRole('button', { name: 'Move up Radarr / Sonarr' });
    const down = screen.getByRole('button', { name: 'Move down Radarr / Sonarr' });
    expect((up as HTMLButtonElement).disabled).toBe(true);
    expect((down as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('housekeeping', () => {
  /**
   * The purge is not undoable and its button sits beside two that are — export
   * and import. It asks first, and the question is the guard.
   */
  it('asks before purging, and purges nothing when refused', async () => {
    const purge = vi.spyOn(api, 'purge');
    mount({});
    await openSection('Maintenance');

    await fireEvent.click(await screen.findByRole('button', { name: /purge/i }));

    expect(await answerConfirmation(null)).toBeTruthy();
    expect(purge).not.toHaveBeenCalled();
  });

  it('purges once the question is answered', async () => {
    const purge = vi.spyOn(api, 'purge').mockResolvedValue({
      decisions_removed: 3,
      logs_removed: 2,
      jobs_removed: 1,
      metadata_cache_removed: 0,
      sessions_removed: 0,
    });
    mount({});
    await openSection('Maintenance');

    await fireEvent.click(await screen.findByRole('button', { name: /purge/i }));
    await answerConfirmation();

    await waitFor(() => expect(purge).toHaveBeenCalledTimes(1));
  });
});

describe('the section strip', () => {
  it('opens the section named in the URL', async () => {
    window.location.hash = '#metadata';
    mount({});

    const tab = await screen.findByRole('tab', { name: 'Metadata' });
    expect(tab.getAttribute('aria-selected')).toBe('true');
  });

  it('moves between sections with the arrow keys, not only with Tab', async () => {
    mount({});

    const general = await screen.findByRole('tab', { name: 'General' });
    await fireEvent.keyDown(general, { key: 'ArrowRight' });

    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Routing' }).getAttribute('aria-selected')).toBe(
        'true',
      ),
    );
  });
});

/**
 * An import is the second writer of the settings table, and the screen holds a
 * copy of what it wrote. Both have to be told.
 */
describe('importing a configuration', () => {
  const BUNDLE = {
    settings: 3,
    categories: 1,
    instances: 0,
    root_folders: 0,
    overrides: 0,
    skipped: [],
    needs_key: [],
  };

  async function importFile(contents: object) {
    // The page is a spinner while it reloads, file input included.
    await waitFor(() => expect(document.querySelector('input[type="file"]')).not.toBeNull());
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File([JSON.stringify(contents)], 'routarr-config.json', {
      type: 'application/json',
    });
    await userEvent.upload(input, file);
  }

  it('re-reads the settings it just overwrote', async () => {
    const getSettings = vi.spyOn(api, 'getSettings');
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ ui_theme: 'dark' });
    await openSection('Maintenance');

    const before = getSettings.mock.calls.length;
    await importFile({ version: 1 });

    // Without this the draft still holds the pre-import values, and the payload
    // a save sends is built from all of FIELDS — so the next save undoes the
    // import for every setting at once.
    await waitFor(() => expect(getSettings.mock.calls.length).toBeGreaterThan(before));
  });

  /**
   * An import is a save of every setting at once, and a save applies the
   * theme; left to the next reload, the screen kept the old one.
   */
  it('applies the imported theme at once', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ ui_theme: 'dark' });
    await openSection('Maintenance');
    document.documentElement.dataset.theme = 'dark';

    vi.spyOn(api, 'getSettings').mockResolvedValue({ ui_theme: 'light' });
    await importFile({ version: 1 });

    await waitFor(() => expect(document.documentElement.dataset.theme).toBe('light'));
    delete document.documentElement.dataset.theme;
  });

  it('tells the shell its warning count is stale', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({});
    await openSection('Maintenance');

    const before = statusRevision();
    await importFile({ version: 1 });

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /** The import wrote every setting, so an edit made before it is replaced, not kept. */
  it('replaces an unsaved edit with the values it wrote', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ global_dry_run: 'true', batch_limit: '50' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    await openSection('Maintenance');
    vi.mocked(api.getSettings).mockResolvedValue({ global_dry_run: 'true', batch_limit: '25' });

    await importFile({ version: 1 });

    await screen.findByText('Restored: 3 settings.');
    await openSection('Routing');
    await waitFor(() =>
      expect((screen.getByLabelText('Batch limit') as HTMLInputElement).value).toBe('25'),
    );
    expect((screen.getByLabelText('Global dry-run') as HTMLSelectElement).value).toBe('true');
    expect(screen.queryByText('Unsaved changes: 1')).toBeNull();
  });

  /**
   * The same when the settings cannot be read back at once: the edit carried
   * over the values a later Retry reads would be written over the import.
   */
  it('drops an edit made before it even when the read after it fails', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    await openSection('Maintenance');
    vi.mocked(api.getSettings)
      .mockRejectedValueOnce(new ApiError('The settings could not be read', 409, 'conflict'))
      .mockResolvedValue({ global_dry_run: 'true' });

    await importFile({ version: 1 });
    await screen.findByText('The settings could not be read');
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(screen.queryByText('The settings could not be read')).toBeNull());
    await openSection('Routing');
    expect((screen.getByLabelText('Global dry-run') as HTMLSelectElement).value).toBe('true');
    expect(screen.queryByText('Unsaved changes: 1')).toBeNull();
  });

  /** An instance restored without its key is restored: amber and named, never a refusal. */
  it('names the instances waiting for their API key without calling them refused', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue({
      ...BUNDLE,
      instances: 2,
      needs_key: ['Radarr', 'Sonarr'],
    });
    mount({});
    await openSection('Maintenance');

    await importFile({ version: 1 });

    const summary = await screen.findByText(
      'Restored: 3 settings. Instances waiting for their API key: Radarr, Sonarr',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  /** Part restored and part refused is a partial result, each refusal on a line of its own. */
  it('reports what it could not restore, until the next import', async () => {
    const importConfig = vi
      .spyOn(api, 'importConfig')
      .mockResolvedValueOnce({ ...BUNDLE, skipped: ['instance "Radarr": unknown type'] })
      .mockResolvedValueOnce(BUNDLE);
    mount({});
    await openSection('Maintenance');

    await importFile({ version: 1 });

    const summary = await screen.findByText('Restored: 3 settings. Not restored: 1');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText('instance "Radarr": unknown type')).toBeTruthy();

    await importFile({ version: 1 });

    await waitFor(() => expect(importConfig).toHaveBeenCalledTimes(2));
    await screen.findByText('Restored: 3 settings.');
    expect(screen.queryByText('instance "Radarr": unknown type')).toBeNull();
  });

  it('reports an import that restored nothing as a failure', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue({
      settings: 0,
      categories: 0,
      instances: 0,
      root_folders: 0,
      overrides: 0,
      skipped: ['setting "colour": unknown'],
      needs_key: [],
    });
    mount({});
    await openSection('Maintenance');

    await importFile({ version: 1 });

    const summary = await screen.findByText('Restored: 0 settings. Not restored: 1');
    expect(summary.closest('.banner')?.classList.contains('banner-danger')).toBe(true);
  });
});

/**
 * The card belongs on the screen only where the middleware reads a key. `none`
 * and `external` resolve an identity before they ever look at the header, so a
 * field there writes a string nothing will read — and where a key can be
 * rotated, the card is also where one is minted.
 */
describe('the API key card', () => {
  it('is offered in the mode whose only credential it is', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    expect(await screen.findByLabelText('Routarr API key')).toBeInTheDocument();
    expect(screen.getByText('Generated at first start')).toBeInTheDocument();
  });

  it.each([['none' as const], ['external' as const]])(
    'is absent in %s mode, where nothing reads the header',
    async (mode) => {
      mount({}, { mode, api_key_configured: true, api_key_pinned: false });

      await screen.findByRole('heading', { name: 'Settings', level: 1 });
      expect(screen.queryByLabelText('Routarr API key')).toBeNull();
    },
  );

  /// The middleware tries the key before the session, so it still opens the
  /// door — and the wording has to stop claiming the key is generated, which in
  /// these modes it is not.
  it('is offered in a session mode that has a key, and says what it is for', async () => {
    mount({}, { mode: 'forms', api_key_configured: true, api_key_pinned: false });

    expect(await screen.findByLabelText('Routarr API key')).toBeInTheDocument();
    expect(screen.getByText('For clients that cannot hold a session')).toBeInTheDocument();
    expect(screen.queryByText('Generated at first start')).toBeNull();
  });

  /// Nothing to paste, but something to create: this is how a script gets a
  /// credential without being handed the password.
  it('offers to mint one in a session mode that has none, and nothing to paste', async () => {
    mount({}, { mode: 'oidc', api_key_configured: false, api_key_pinned: false });

    expect(await screen.findByRole('button', { name: 'Create a key' })).toBeInTheDocument();
    expect(screen.queryByLabelText('Routarr API key')).toBeNull();
  });

  /// A key the environment states cannot be replaced from here: the new one
  /// would last until the next restart. A button that quietly expires is worse
  /// than no button.
  it('offers no button when the environment pins the key, and says why', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: true });

    expect(await screen.findByText('The environment sets it')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Regenerate' })).toBeNull();
  });

  it('stores what it just minted, since this browser is a client too', async () => {
    const rotate = vi.spyOn(api, 'rotateApiKey').mockResolvedValue({ api_key: 'the-new-one' });
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Regenerate' }));
    // The question names what breaks, not just what is about to happen.
    expect(await answerConfirmation()).toBe('Regenerate the key?');

    await waitFor(() => expect(rotate).toHaveBeenCalled());
    // Shown once, and kept where the next request will find it — in `apikey`
    // mode the browser was holding the key that just stopped working.
    expect(await screen.findByText('the-new-one')).toBeInTheDocument();
    expect(localStorage.getItem('routarr.apiKey')).toBe('the-new-one');
  });

  it('asks before replacing a key other clients are using', async () => {
    const rotate = vi.spyOn(api, 'rotateApiKey');
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Regenerate' }));
    await answerConfirmation(null);

    expect(rotate).not.toHaveBeenCalled();
  });

  /// Removing it in `apikey` mode would lock everybody out, so the control is
  /// not there at all — the backend refuses too, and neither relies on the other.
  it('offers removal only where another way in remains', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });
    await screen.findByRole('button', { name: 'Regenerate' });
    expect(screen.queryByRole('button', { name: 'Remove the key' })).toBeNull();

    cleanup();
    mount({}, { mode: 'forms', api_key_configured: true, api_key_pinned: false });
    expect(await screen.findByRole('button', { name: 'Remove the key' })).toBeInTheDocument();
  });
});

/**
 * The backend bounds every number and refuses a payload outside them — after
 * Save, naming a key in a tab the operator may never have opened. The field
 * says so first, and Save waits.
 */
describe('a number outside its bounds', () => {
  it('disables Save and marks the field, until it is back in range', async () => {
    mount({ batch_limit: '50' });
    await openSection('Routing');

    const field = (await screen.findByLabelText('Batch limit')) as HTMLInputElement;
    expect(field.getAttribute('min')).toBe('1');
    expect(field.getAttribute('max')).toBe('1000');

    // Save appears with the first change, and refuses this one.
    await fireEvent.input(field, { target: { value: '0' } });
    const save = (await screen.findByRole('button', { name: 'Save' })) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(field.getAttribute('aria-invalid')).toBe('true');

    await fireEvent.input(field, { target: { value: '25' } });
    expect(save.disabled).toBe(false);
    expect(field.hasAttribute('aria-invalid')).toBe(false);
  });
});

describe('a refused save', () => {
  /**
   * A write that failed is not a load to try again. Offered a Retry, the
   * refusal would reload the settings and reseed the form, and every unsaved
   * change in every section would go with it.
   */
  it('offers no retry, and keeps every edit', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    vi.spyOn(api, 'updateSettings').mockRejectedValue(
      new ApiError('The batch limit is out of range', 400, 'bad_request'),
    );

    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(await screen.findByText('The batch limit is out of range')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
    expect((screen.getByLabelText('Global dry-run') as HTMLSelectElement).value).toBe('false');
    expect(screen.getByText('Unsaved changes: 1')).toBeTruthy();
  });

  /** Save waits for a read that succeeded, and only Retry gives it one. */
  it('holds Save while the settings cannot be read', async () => {
    mount({ global_dry_run: 'true' });
    vi.mocked(api.getSettings)
      .mockReset()
      .mockRejectedValueOnce(new ApiError('The settings could not be read', 409, 'conflict'))
      .mockResolvedValue({ global_dry_run: 'true' });
    cleanup();
    renderWithI18n(Settings, { strings: STRINGS });
    await screen.findByText('The settings could not be read');
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');

    expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole('button', { name: 'Dismiss' })).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() =>
      expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
  });

  /**
   * A reload after a failed load takes the stored values and keeps what was
   * edited meanwhile. Reseeded, the edit is lost. Kept alone, every field the
   * failed load never delivered is saved as its fallback.
   */
  it('keeps an edit across a reload, over the values it reads', async () => {
    mount({ global_dry_run: 'true', batch_limit: '25' });
    vi.mocked(api.getSettings)
      .mockReset()
      .mockRejectedValueOnce(new ApiError('The settings could not be read', 409, 'conflict'))
      .mockResolvedValue({ global_dry_run: 'true', batch_limit: '25' });
    cleanup();
    renderWithI18n(Settings, { strings: STRINGS });
    await screen.findByText('The settings could not be read');
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.queryByText('The settings could not be read')).toBeNull());

    expect((screen.getByLabelText('Global dry-run') as HTMLSelectElement).value).toBe('false');
    const payload = await save();
    expect(payload.global_dry_run).toBe('false');
    expect(payload.batch_limit).toBe('25');
  });
});
