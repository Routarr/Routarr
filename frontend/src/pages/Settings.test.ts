import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { SERVER_COUNTS } from '../test/counts';
import { statusRevision } from '../lib/status.svelte';
import { ApiError, api } from '../api/client';
import type { AuthMode } from '../api/types';
import { FIELDS, SECTIONS } from '../lib/settings';
import Settings from './Settings.svelte';
import Sources from './Sources.svelte';
import { onboarding, publishOnboarding } from '../lib/onboarding.svelte';
import { interceptLinks, navigate, router } from '../lib/router.svelte';
import { onboardingStatus } from '../test/fixtures';
import { withBase } from '../test/base';
import { answerConfirmation } from '../test/confirm';
import { captureDownloads } from '../test/downloads';

// The proof a key needs has its own tests (`lib/proof.test.ts`): here it is
// given, as the dialog would hand it back.
vi.mock('../lib/proof.svelte', () => ({
  withProof: <T>(send: (proof: object) => Promise<T>) => send({ current_key: 'the-key-proven' }),
}));

/**
 * The two screens built on the settings editor: Settings, and the metadata
 * sources, which have a screen of their own.
 *
 * Settings is where unattended writing gets armed, so the warning that says so
 * is a guardrail rather than decoration: neither switch is dangerous alone, and
 * the banner must appear for the combination and only for it.
 */

const STRINGS = {
  Settings: 'Settings',
  RangeBetween: 'Enter a whole number from {min} to {max}.',
  RangeAtLeast: 'Enter a whole number of {min} or more.',
  SectionHasInvalid: '{section}: a value is outside its bounds',
  SaveHeldInvalid: 'Save waits for every marked value to be within its bounds.',
  LiveModeWarning: 'Live mode is on',
  AutoApplyWarning: 'Automatic application is armed',
  SettingsSaved: 'Saved',
  SettingsSections: 'Sections',
  SettingsTabRouting: 'Routing',
  SettingBatchLimit: 'Batch limit',
  SettingLogRetention: 'Log retention',
  SettingsTabAutomation: 'Automation',
  SettingsTabNotifications: 'Notifications',
  Metadata: 'Metadata',
  MetadataSources: 'Metadata sources',
  SettingMetadataTtl: 'Cache lifetime',
  SettingsTabGeneral: 'General',
  GuideTitle: 'Getting started',
  GuideRestartText: 'Show the steps again.',
  GuideRestart: 'Show the guide',
  GuideLiveAction: 'Open the routing settings',
  GuideLiveTitle: 'Turn off the global dry-run',
  GuideOptionalDoneNext: 'Optional step done. Next: {next}',
  SettingsTabMaintenance: 'Maintenance',
  UnsavedChanges: 'Unsaved changes: {count}',
  ConfirmLeaveUnsaved: 'Leave without saving? Unsaved changes: {count}',
  SavesEverySection: 'Every section is saved together',
  DiscardChanges: 'Discard',
  Save: 'Save',
  SettingNotificationFormat: 'Notification format',
  SettingOn: 'On',
  SettingOff: 'Off',
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
  ExportConfig: 'Export configuration',
  ConfirmPurge: 'Remove everything past its retention?',
  PurgeResult: '{decisions} decisions, {logs} logs, {jobs} jobs removed',
  Backups: 'Backups',
  NoBackupsYet: 'No backup yet',
  BackupNow: 'Back up now',
  SettingGlobalDryRun: 'Global dry-run',
  SettingAutoApply: 'Apply automatically',
  SecretConfiguredPlaceholder: 'A key is stored – type to replace it',
  SecretStoredPlaceholder: 'A value is stored – type to replace it',
  SecretRemovedOnSave: 'Removed when you save',
  SettingNotificationWebhook: 'Notification webhook',
  Remove: 'Remove',
  ConfigImportResult: 'Restored: {settings} settings.',
  ImportReplaceQuestion: 'Replace the rules, or add to them?',
  ImportAppend: 'Add',
  ImportReplace: 'Replace',
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
  settings: Record<string, string | boolean>,
  auth: AuthMode = APIKEY_MODE,
  provider: ProviderOverride = {},
  page: typeof Settings = Settings,
) {
  vi.spyOn(api, 'authMode').mockResolvedValue(auth);
  vi.spyOn(api, 'getSettings').mockResolvedValue(settings);
  vi.spyOn(api, 'getCategories').mockResolvedValue([]);
  vi.spyOn(api, 'getLanguages').mockResolvedValue({
    default: 'en',
    languages: [{ code: 'en', name: 'English', completion: 100, direction: 'ltr' }],
  });
  // The backups card loads on mount, and unmocked it would reach the network.
  vi.spyOn(api, 'listBackups').mockResolvedValue({ backups: [], retention_count: 7 });
  // So does the sessions card, in every mode a browser signs in.
  vi.spyOn(api, 'getSessions').mockResolvedValue([]);
  // The signing card loads with the Automation section, and its failure would
  // pass through every test of that section unseen.
  vi.spyOn(api, 'webhookSigning').mockResolvedValue({ signed: false, since: null, readable: true });
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
        media_types: ['movie', 'series'],
      },
      {
        id: 'tmdb',
        display_name: 'TMDB',
        fetched: true,
        needs_key: true,
        key_env: 'TMDB_API_KEY',
        configured: provider.configured ?? false,
        fields: ['genres', 'keywords'],
        media_types: ['movie', 'series'],
      },
    ],
    order: provider.order ?? ['arr'],
  });
  return renderWithI18n(page, { strings: STRINGS });
}

/** The metadata sources screen, with the same stand-ins as Settings. */
const mountSources = (
  settings: Record<string, string | boolean>,
  provider: ProviderOverride = {},
) => mount(settings, APIKEY_MODE, provider, Sources);

/** The keys of the fields a screen edits. */
const fieldsOf = (page: string) =>
  SECTIONS.filter((section) => section.page === page).flatMap(
    (section) => section.keys as readonly string[],
  );

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
    counts: [...SERVER_COUNTS],
  });
  await fireEvent.click(await screen.findByRole('button', { name: 'Save' }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  return nthCall(update)[0] as Record<string, string>;
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  withBase(null);
  publishOnboarding(null);
});

describe('the unattended-writing warning', () => {
  it('stays quiet while dry run is on', async () => {
    mount({ global_dry_run: 'true', auto_apply_enabled: 'true' });

    // The tabs, not the heading: the heading draws before the settings load,
    // and an absence checked then holds whatever the loaded screen does.
    await screen.findByRole('tab', { name: 'Routing' });
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

  /** Said where the two switches are, not on a screen that cannot turn them off. */
  it('stays off the sources screen', async () => {
    mountSources({ global_dry_run: 'false', auto_apply_enabled: 'true' });

    await screen.findByText('Active, in priority order');
    expect(screen.queryByText('Live mode is on')).toBeNull();
    expect(screen.queryByText('Automatic application is armed')).toBeNull();
  });
});

describe('the save bar', () => {
  it('is absent until something is edited', async () => {
    mount({ global_dry_run: 'true' });

    await screen.findByRole('tab', { name: 'Routing' });
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
  });

  /**
   * Saving covers every field of every section of the screen, so the button
   * has to say so. Sitting inside whichever section is open, it says neither
   * that nor that there are unsaved changes at all.
   */
  it('appears with a count once a field changes, and says what it covers', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');

    expect(await screen.findByText('Unsaved changes: 1')).toBeTruthy();
    expect(screen.getByText('Every section is saved together')).toBeTruthy();
  });

  it('sends every field of the screen, and none of the other screen', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    const payload = await save();
    // Every field except the credentials, which are blank here and are left
    // out rather than sent: see the metadata sources below for why.
    const credentials = new Set(FIELDS.filter((f) => f.kind === 'secret').map((f) => f.key));
    expect(Object.keys(payload).sort()).toEqual(
      fieldsOf('settings')
        .filter((key) => !credentials.has(key))
        .sort(),
    );
  });

  /**
   * One section, so no strip to choose from and nothing else saved with it:
   * a draft left on Settings stays a draft.
   */
  it('saves the sources alone, from a screen with no section strip', async () => {
    mountSources({ metadata_providers: 'arr' });

    await fireEvent.input(await screen.findByLabelText('Cache lifetime'), {
      target: { value: '14' },
    });
    expect(screen.queryByRole('tab')).toBeNull();
    expect(screen.queryByText('Every section is saved together')).toBeNull();
    const credentials = new Set(FIELDS.filter((f) => f.kind === 'secret').map((f) => f.key));
    expect(Object.keys(await save()).sort()).toEqual(
      fieldsOf('sources')
        .filter((key) => !credentials.has(key))
        .sort(),
    );
  });

  /**
   * A metadata key saved or cleared is what the source warnings are computed
   * from, so saving has to tell the shell to read them again.
   */
  it('tells the shell its counters are out of date', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');

    const before = statusRevision();
    await save();

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /** Leaving drops the draft the bar counts, so the reader is asked first. */
  it('asks before leaving with unsaved changes, and stays on Cancel', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    await screen.findByText('Unsaved changes: 1');

    navigate('/rules');
    expect(await answerConfirmation(null)).toBe('Leave without saving? Unsaved changes: 1');

    expect(router.path).not.toBe('/rules');
    expect(screen.getByText('Unsaved changes: 1')).toBeTruthy();
  });

  it('leaves once the reader agrees to drop the changes', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    await screen.findByText('Unsaved changes: 1');

    navigate('/rules');
    await answerConfirmation();

    expect(router.path).toBe('/rules');
  });

  it('leaves without a question when nothing is pending', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');

    navigate('/rules');

    await waitFor(() => expect(router.path).toBe('/rules'));
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('holds the tab open while changes are pending, and only then', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    const unloading = () => {
      const event = new Event('beforeunload', { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(unloading()).toBe(false);

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    await screen.findByText('Unsaved changes: 1');

    expect(unloading()).toBe(true);
  });

  it('puts the draft back when the change is discarded', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');

    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    await screen.findByText('Unsaved changes: 1');
    await fireEvent.click(screen.getByRole('button', { name: 'Discard' }));

    await waitFor(() => expect(screen.queryByText('Unsaved changes: 1')).toBeNull());
  });

  /**
   * Save and Discard live in the bar, which goes once nothing is pending, and
   * the focus with it. The open section takes it back.
   */
  it('hands the focus to the open section once a save takes the bar away', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    (await screen.findByRole('button', { name: 'Save' })).focus();

    await save();

    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('tabpanel')));
  });

  it('hands the focus to the open section once a discard takes the bar away', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    const discard = await screen.findByRole('button', { name: 'Discard' });
    discard.focus();

    await fireEvent.click(discard);

    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('tabpanel')));
  });

  /** A save sent by Enter from a field takes nothing away: the focus stays there. */
  it('leaves the focus in the field a save was sent from', async () => {
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(undefined as never);
    vi.spyOn(api, 'getLocalization').mockResolvedValue({
      language: 'en',
      direction: 'ltr',
      strings: STRINGS,
      counts: [...SERVER_COUNTS],
    });
    mount({ batch_limit: '50' });
    await openSection('Routing');
    const field = await screen.findByLabelText('Batch limit');
    await fireEvent.input(field, { target: { value: '25' } });
    field.focus();

    await fireEvent.submit(field.closest('form') as HTMLFormElement);

    await waitFor(() => expect(screen.queryByRole('button', { name: 'Save' })).toBeNull());
    expect(update).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(field);
  });
});

describe('the metadata sources', () => {
  /**
   * The credential lives in the row of the source it unlocks. A key is not a
   * setting of the application but a property of a source, and stated anywhere
   * else, turning a source on takes two saves for one intention.
   */
  it('offers the credential in the row of the source that needs it', async () => {
    mountSources({ metadata_providers: 'arr' });

    const field = await screen.findByLabelText('TMDB');
    expect(field.getAttribute('id')).toBe('setting-tmdb_api_key');
    // The variable is named once, by the field that accepts it.
    expect(field.getAttribute('placeholder')).toBe('API key, or the TMDB_API_KEY variable');
  });

  /**
   * A source that needs a key it has not got is shown, but never as if it
   * worked: putting one at position 1 looks like a configuration and behaves
   * like an absence.
   */
  it('refuses to add a source that cannot answer, until a key is given', async () => {
    mountSources({ metadata_providers: 'arr' });

    const enable = (await screen.findByRole('button', {
      name: 'Enable – TMDB',
    })) as HTMLButtonElement;
    expect(enable.disabled).toBe(true);

    // A key typed but not yet saved counts: refusing the click then would send
    // the reader back for a save they cannot see the need for.
    await userEvent.type(screen.getByLabelText('TMDB'), 'a-key');
    expect(enable.disabled).toBe(false);
  });

  /**
   * The field is empty on every load (the backend never returns a sealed
   * value), so sending it would write that emptiness. Saving an unrelated
   * setting would delete all three credentials with nothing on screen saying
   * so: the sources would simply stop answering on the next pass.
   */
  it('does not delete a stored key when an unrelated setting is saved', async () => {
    mountSources(
      { metadata_cache_ttl_days: '30', metadata_providers: 'arr,tmdb' },
      { configured: true, order: ['arr', 'tmdb'] },
    );

    await fireEvent.input(await screen.findByLabelText('Cache lifetime'), {
      target: { value: '14' },
    });
    const payload = await save();
    expect(payload).not.toHaveProperty('tmdb_api_key');
    // What was actually edited still goes.
    expect(payload.metadata_cache_ttl_days).toBe('14');
  });

  it('sends a credential the reader did type', async () => {
    mountSources({ metadata_providers: 'arr' });

    await userEvent.type(await screen.findByLabelText('TMDB'), 'a-key');
    expect((await save()).tmdb_api_key).toBe('a-key');
  });

  /**
   * A setting the form shows is a setting the form sends.
   *
   * "Blank means leave the stored value alone" exists for one reason: the
   * backend never returns a sealed credential, so that field is empty on every
   * load and sending the emptiness would delete the key. The rule holds for a
   * secret and for nothing else. Applied to the source list, it would drop the
   * one setting whose empty value means something, show "Settings saved", and
   * leave the sources answering exactly as before.
   *
   * An empty list is refused by the backend, which is the point: the operator
   * is told the list is invalid instead of being told it was saved.
   */
  it('sends an emptied source list rather than dropping it', async () => {
    // A stored order without `arr`, so its last source carries the button the
    // Arr's own row deliberately does not.
    mountSources({ metadata_providers: 'tmdb' }, { order: ['tmdb'] });

    await fireEvent.click(await screen.findByRole('button', { name: 'Disable – TMDB' }));
    const payload = await save();
    expect(payload).toHaveProperty('metadata_providers');
    expect(payload.metadata_providers).toBe('');
  });

  /**
   * The shipped order is the backend's to state. A copy kept here drifts from
   * it, and since a save sends every field of the screen, the first save of
   * anything on it would store the copy. A list nobody chose is the server's.
   */
  it('shows and saves the order the server resolved when none is stored', async () => {
    mountSources({ metadata_cache_ttl_days: '30' }, { configured: true, order: ['tmdb', 'arr'] });

    await fireEvent.input(await screen.findByLabelText('Cache lifetime'), {
      target: { value: '14' },
    });
    expect((await save()).metadata_providers).toBe('tmdb,arr');
  });

  /**
   * The server refuses a source listed twice, but a stored list can still
   * arrive here with one. Each row is keyed by its source, and a save sends
   * every field of the screen, so the list is read without the repeat and
   * saved that way.
   */
  it('reads a stored list without its repeated source, and saves it that way', async () => {
    mountSources(
      { metadata_cache_ttl_days: '30', metadata_providers: 'tmdb,arr,tmdb' },
      { configured: true, order: ['tmdb', 'arr'] },
    );
    expect(await screen.findAllByRole('button', { name: 'Disable – TMDB' })).toHaveLength(1);

    await fireEvent.input(screen.getByLabelText('Cache lifetime'), { target: { value: '14' } });
    expect((await save()).metadata_providers).toBe('tmdb,arr');
  });

  it('separates what is active from what is switched off', async () => {
    mountSources({ metadata_providers: 'arr' });

    expect(await screen.findByText('Active, in priority order')).toBeTruthy();
    // Not "Available", which would name exactly the sources that are not: a
    // keyless one can answer nothing at all.
    expect(screen.getByText('Inactive')).toBeTruthy();
  });

  it('cannot move the only active source', async () => {
    mountSources({ metadata_providers: 'arr' });

    const up = await screen.findByRole('button', { name: 'Move up – Radarr / Sonarr' });
    const down = screen.getByRole('button', { name: 'Move down – Radarr / Sonarr' });
    expect((up as HTMLButtonElement).disabled).toBe(true);
    expect((down as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('the notification webhook', () => {
  it('offers every format the server writes, and saves the one chosen', async () => {
    mount({});
    await openSection('Notifications');

    const format = await screen.findByLabelText('Notification format');
    const offered = [...format.querySelectorAll('option')].map((option) => option.value);
    expect(offered).toEqual(['auto', 'json', 'discord', 'ntfy', 'gotify', 'apprise']);
    expect((format as HTMLSelectElement).value).toBe('auto');

    await userEvent.selectOptions(format, 'ntfy');
    expect((await save()).notification_format).toBe('ntfy');
  });

  /**
   * The address is the channel's credential, sealed by the server and never
   * returned: the field reads empty whether or not one is stored, so the
   * placeholder says which, and a save that leaves it blank leaves it alone.
   */
  it('says an address is stored, and leaves it alone when saved blank', async () => {
    mount({ notification_webhook_url_configured: true });
    await openSection('Notifications');

    const field = await screen.findByLabelText('Notification webhook');
    expect(field.getAttribute('type')).toBe('password');
    expect(field.getAttribute('placeholder')).toBe('A value is stored – type to replace it');

    await userEvent.selectOptions(screen.getByLabelText('Notification format'), 'ntfy');
    const payload = await save();
    expect(payload).not.toHaveProperty('notification_webhook_url');
    expect(payload.notification_format).toBe('ntfy');
  });

  /** Blank means "leave it", so removing one takes a button of its own. */
  it('removes a stored address at the next save', async () => {
    mount({ notification_webhook_url_configured: true });
    await openSection('Notifications');

    await fireEvent.click(
      await screen.findByRole('button', { name: 'Remove – Notification webhook' }),
    );

    // The button goes, so the field takes the focus and says what Save will do.
    const field = screen.getByLabelText('Notification webhook');
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(field.getAttribute('placeholder')).toBe('Removed when you save');
    expect((await save()).notification_webhook_url).toBe('');
  });

  it('offers nothing to remove while no address is stored', async () => {
    mount({ notification_webhook_url_configured: false });
    await openSection('Notifications');

    await screen.findByLabelText('Notification webhook');
    expect(screen.queryByRole('button', { name: 'Remove – Notification webhook' })).toBeNull();
  });
});

/** A setting's state agrees as a setting does, not as the rule or instance badges do. */
it('offers a switch in words of its own', async () => {
  mount({ auto_apply_enabled: 'false' });
  await openSection('Automation');

  const options = [...(screen.getByLabelText('Apply automatically') as HTMLSelectElement).options];
  expect(options.map((option) => option.textContent?.trim())).toEqual(['On', 'Off']);
});

/**
 * A source's key lives in the source's row, where a blank field keeps the
 * stored one: without a Remove there, a key stays sealed behind a disabled
 * source for good.
 */
describe("a source's stored key", () => {
  it('is removed at the next save', async () => {
    mountSources({ metadata_providers: 'arr', tmdb_api_key_configured: true });

    await fireEvent.click(await screen.findByRole('button', { name: 'Remove – TMDB' }));

    const field = screen.getByLabelText('TMDB');
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(field.getAttribute('placeholder')).toBe('Removed when you save');
    expect(screen.queryByRole('button', { name: 'Remove – TMDB' })).toBeNull();
    expect((await save()).tmdb_api_key).toBe('');
  });

  it('offers nothing to remove while no key is stored', async () => {
    mountSources({ metadata_providers: 'arr' });

    await screen.findByLabelText('TMDB');
    expect(screen.queryByRole('button', { name: 'Remove – TMDB' })).toBeNull();
  });
});

/** The configuration a reader versions or carries over, readable, under a name of its own. */
it('exports the configuration as a readable file', async () => {
  vi.spyOn(api, 'exportConfig').mockResolvedValue({ version: 1, settings: { batch_limit: '25' } });
  const saved = captureDownloads();
  mount({});
  await openSection('Maintenance');

  await fireEvent.click(await screen.findByRole('button', { name: 'Export configuration' }));

  await waitFor(() => expect(saved).toHaveLength(1));
  expect(saved[0]?.name).toBe('routarr-config.json');
  expect(JSON.parse(await saved[0]!.blob.text())).toEqual({
    version: 1,
    settings: { batch_limit: '25' },
  });
});

describe('housekeeping', () => {
  /**
   * The purge is not undoable and its button sits beside two that are (export
   * and import). It asks first, and the question is the guard.
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
      source_identifiers_removed: 0,
      sessions_removed: 0,
      security_events_removed: 0,
    });
    mount({});
    await openSection('Maintenance');
    const before = statusRevision();

    await fireEvent.click(await screen.findByRole('button', { name: /purge/i }));
    await answerConfirmation();

    await waitFor(() => expect(purge).toHaveBeenCalledTimes(1));
    // The navigation counts pending decisions, some of which the purge removed.
    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });
});

describe('the section strip', () => {
  /** A bare fragment resolves against the `<base href>`, which is the mount point. */
  it('keeps the page address when switching sections under a mount point', async () => {
    withBase('/routarr/');
    window.history.replaceState({}, '', '/routarr/settings');
    mount({});

    await openSection('Routing');

    expect(window.location.pathname).toBe('/routarr/settings');
    expect(window.location.hash).toBe('#routing');
  });

  it('opens the section named in the URL', async () => {
    window.history.replaceState({}, '', '/settings#notifications');
    mount({});

    const tab = await screen.findByRole('tab', { name: 'Notifications' });
    expect(tab.getAttribute('aria-selected')).toBe('true');
  });

  /** The sources are a screen of their own, so their old fragment opens nothing here. */
  it('opens the first section for a fragment no section of the screen holds', async () => {
    window.history.replaceState({}, '', '/settings#metadata');
    mount({});

    const tab = await screen.findByRole('tab', { name: 'General' });
    expect(tab.getAttribute('aria-selected')).toBe('true');
    expect(screen.queryByRole('tab', { name: 'Metadata' })).toBeNull();
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
  /** The restore is the disaster path, and a keyboard has to reach it. */
  it('offers the import as a button that opens the file picker', async () => {
    mount({});
    await openSection('Maintenance');
    const pick = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(() => {});

    await fireEvent.click(await screen.findByRole('button', { name: 'ImportConfig' }));
    expect(pick).toHaveBeenCalledTimes(1);
  });

  const BUNDLE = {
    settings: 3,
    categories: 1,
    instances: 0,
    root_folders: 0,
    overrides: 0,
    rules: 0,
    skipped: [],
    needs_key: [],
    adjusted: [],
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

  /**
   * A bundle carrying rules asks what the Rules screen asks of a rule file,
   * and sends the answer: Cancel imports nothing.
   */
  it('asks whether the rules of a bundle replace the ones in place', async () => {
    const importConfig = vi.spyOn(api, 'importConfig').mockResolvedValue({ ...BUNDLE, rules: 1 });
    mount({});
    await openSection('Maintenance');
    const withRules = { version: 1, rules: [{ name: 'Anime' }] };

    await importFile(withRules);
    expect(await answerConfirmation(null)).toBe('Replace the rules, or add to them?');
    await importFile(withRules);
    await answerConfirmation('replace');
    await waitFor(() => expect(importConfig).toHaveBeenCalledTimes(1));
    expect(importConfig).toHaveBeenLastCalledWith(withRules, true);

    await importFile({ version: 1 });
    await waitFor(() => expect(importConfig).toHaveBeenCalledTimes(2));
    expect(importConfig).toHaveBeenLastCalledWith({ version: 1 }, false);
  });

  it('re-reads the settings it just overwrote', async () => {
    const getSettings = vi.spyOn(api, 'getSettings');
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ ui_theme: 'dark' });
    await openSection('Maintenance');

    const before = getSettings.mock.calls.length;
    await importFile({ version: 1 });

    // Without this the draft still holds the pre-import values, and the payload
    // a save sends is built from all of FIELDS, so the next save undoes the
    // import for every setting at once.
    await waitFor(() => expect(getSettings.mock.calls.length).toBeGreaterThan(before));
  });

  /**
   * An import is a save of every setting at once, and a save applies the
   * theme. Left to the next reload, the screen would keep the old one.
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
      rules: 0,
      skipped: ['setting "colour": unknown'],
      needs_key: [],
      adjusted: [],
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
 * field there writes a string nothing will read. Where a key can be rotated,
 * the card is also where one is minted.
 */
describe('the API key card', () => {
  it('is offered in the mode whose only credential it is', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    expect(await screen.findByRole('heading', { name: 'Routarr API key' })).toBeInTheDocument();
    expect(screen.getByText('Generated at first start')).toBeInTheDocument();
  });

  it.each([['none' as const], ['external' as const]])(
    'is absent in %s mode, where nothing reads the header',
    async (mode) => {
      mount({}, { mode, api_key_configured: true, api_key_pinned: false });

      await screen.findByRole('heading', { name: 'Settings', level: 1 });
      expect(screen.queryByRole('heading', { name: 'Routarr API key' })).toBeNull();
    },
  );

  /**
   * The middleware tries the key before the session, so it still opens the
   * door, and the wording must not claim the key is generated, which in these
   * modes it is not.
   */
  it('is offered in a session mode that has a key, and says what it is for', async () => {
    mount({}, { mode: 'forms', api_key_configured: true, api_key_pinned: false });

    expect(await screen.findByRole('heading', { name: 'Routarr API key' })).toBeInTheDocument();
    expect(screen.getByText('For clients that cannot hold a session')).toBeInTheDocument();
    expect(screen.queryByText('Generated at first start')).toBeNull();
  });

  /**
   * Something to create: this is how a script gets a credential without being
   * handed the password.
   */
  it('offers to mint one in a session mode that has none', async () => {
    mount({}, { mode: 'oidc', api_key_configured: false, api_key_pinned: false });

    expect(await screen.findByRole('button', { name: 'Create a key' })).toBeInTheDocument();
  });

  /**
   * A key the environment states cannot be replaced from here: the new one
   * would last until the next restart. A button that quietly expires is worse
   * than no button.
   */
  it('offers no button when the environment pins the key, and says why', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: true });

    expect(await screen.findByText('The environment sets it')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Regenerate' })).toBeNull();
  });

  /**
   * Shown once, with the proof sent, and kept nowhere: this browser holds a
   * session, not the key.
   */
  it('shows what it just minted, once, and keeps no copy', async () => {
    const rotate = vi.spyOn(api, 'rotateApiKey').mockResolvedValue({ api_key: 'the-new-one' });
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Regenerate' }));
    // The question names what breaks, not just what is about to happen.
    expect(await answerConfirmation()).toBe('Regenerate the key?');

    await waitFor(() => expect(rotate).toHaveBeenCalledWith({ current_key: 'the-key-proven' }));
    expect(await screen.findByText('the-new-one')).toBeInTheDocument();
    expect(localStorage.getItem('routarr.apiKey')).toBeNull();
  });

  it('asks before replacing a key other clients are using', async () => {
    const rotate = vi.spyOn(api, 'rotateApiKey');
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Regenerate' }));
    await answerConfirmation(null);

    expect(rotate).not.toHaveBeenCalled();
  });

  /**
   * Removing it in `apikey` mode would lock everybody out, so the control is
   * not there at all. The backend refuses too, and neither relies on the other.
   */
  it('offers removal only where another way in remains', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });
    await screen.findByRole('button', { name: 'Regenerate' });
    expect(screen.queryByRole('button', { name: 'Remove the key' })).toBeNull();

    cleanup();
    mount({}, { mode: 'forms', api_key_configured: true, api_key_pinned: false });
    expect(await screen.findByRole('button', { name: 'Remove the key' })).toBeInTheDocument();
  });

  it('keeps the key when its removal is cancelled', async () => {
    const remove = vi.spyOn(api, 'deleteApiKey');
    mount({}, { mode: 'forms', api_key_configured: true, api_key_pinned: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Remove the key' }));
    expect(await answerConfirmation(null)).toBe('Remove the key?');

    expect(remove).not.toHaveBeenCalled();
  });

  it('removes the key once confirmed, with the proof sent', async () => {
    const remove = vi.spyOn(api, 'deleteApiKey').mockResolvedValue(undefined as never);
    mount({}, { mode: 'forms', api_key_configured: true, api_key_pinned: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Remove the key' }));
    await answerConfirmation();

    await waitFor(() => expect(remove).toHaveBeenCalledWith({ current_key: 'the-key-proven' }));
  });
});

/**
 * The backend bounds every number and refuses a payload outside them, but only
 * after Save, naming a key in a tab the operator may never have opened. The
 * field says so first, and Save waits.
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

  /**
   * A value outside its bounds holds Save. The field names its bounds and the
   * save bar says why Save waits, or nothing on screen says which field holds
   * it, and a screen reader hears "invalid" only back on that very field.
   */
  it('says the bounds under the field, and why Save waits', async () => {
    mount({ batch_limit: '50' });
    await openSection('Routing');
    const field = await screen.findByLabelText('Batch limit');

    await fireEvent.input(field, { target: { value: '0' } });

    expect(screen.getByText('Enter a whole number from 1 to 1000.')).toBeTruthy();
    expect(field).toHaveAccessibleDescription(
      expect.stringContaining('Enter a whole number from 1 to 1000.'),
    );
    expect(screen.getByRole('button', { name: 'Save' })).toHaveAccessibleDescription(
      'Save waits for every marked value to be within its bounds.',
    );
  });

  /** A retention has no ceiling, so its bound is said as a floor. */
  it('says an open-ended bound as a floor', async () => {
    mount({ log_retention_days: '90' });
    await openSection('Maintenance');
    const field = await screen.findByLabelText('Log retention');

    await fireEvent.input(field, { target: { value: '-1' } });

    expect(field).toHaveAccessibleDescription(
      expect.stringContaining('Enter a whole number of 0 or more.'),
    );
  });

  /** From another tab, the field that holds Save is out of sight. */
  it('marks the tab that holds a value outside its bounds', async () => {
    mount({ batch_limit: '50' });
    await openSection('Routing');
    await fireEvent.input(await screen.findByLabelText('Batch limit'), {
      target: { value: '0' },
    });

    await openSection('General');

    expect(
      screen.getByRole('tab', { name: 'Routing: a value is outside its bounds' }),
    ).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'General' })).toBeTruthy();
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

  /** Save guards against a second press while one runs, and a refusal ends the run. */
  it('sends the next save once the refused one is answered', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Routing');
    await userEvent.selectOptions(await screen.findByLabelText('Global dry-run'), 'false');
    const update = vi
      .spyOn(api, 'updateSettings')
      .mockRejectedValueOnce(new ApiError('The batch limit is out of range', 400, 'bad_request'))
      .mockResolvedValueOnce(undefined as never);
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('The batch limit is out of range')).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(2));
    expect(nthCall(update, 1)[0]).toMatchObject({ global_dry_run: 'false' });
    await waitFor(() => expect(screen.queryByText('The batch limit is out of range')).toBeNull());
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

describe('the getting-started guide', () => {
  it('can be shown again once it was skipped', async () => {
    publishOnboarding(onboardingStatus([], { state: 'dismissed' }));
    const resumed = onboardingStatus();
    const set = vi.spyOn(api, 'setOnboarding').mockResolvedValue(resumed);
    window.history.replaceState({}, '', '/settings');
    mount({});

    await fireEvent.click(await screen.findByRole('button', { name: 'Show the guide' }));

    await waitFor(() => expect(set).toHaveBeenCalledWith('pending'));
    await waitFor(() => expect(window.location.pathname).toBe('/'));
    expect(onboarding.current).toEqual(resumed);
  });

  it('follows the guide from the sources screen to the routing settings', async () => {
    const stop = interceptLinks();
    try {
      publishOnboarding(
        onboardingStatus(['instance', 'categories', 'metadata', 'rule', 'simulation']),
      );
      window.history.replaceState({}, '', '/sources');
      mountSources({});

      await fireEvent.click(await screen.findByRole('link', { name: 'Open the routing settings' }));

      await waitFor(() => expect(window.location.pathname).toBe('/settings'));
      expect(window.location.hash).toBe('#routing');
    } finally {
      stop();
    }
  });

  /** A link to another section of the screen on display moves no route, only the fragment. */
  it('opens the section a link names without leaving the screen', async () => {
    window.history.replaceState({}, '', '/settings#general');
    mount({});
    await screen.findByRole('tab', { name: 'General' });

    navigate('/settings#routing');

    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Routing' })).toHaveAttribute('aria-selected', 'true'),
    );
  });

  it('leads back to a guide that already shows, without writing anything', async () => {
    publishOnboarding(onboardingStatus());
    const set = vi.spyOn(api, 'setOnboarding');
    window.history.replaceState({}, '', '/settings');
    mount({});

    await fireEvent.click(await screen.findByRole('button', { name: 'Show the guide' }));

    await waitFor(() => expect(window.location.pathname).toBe('/'));
    expect(set).not.toHaveBeenCalled();
  });
});
