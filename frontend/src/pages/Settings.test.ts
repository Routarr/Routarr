import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { nthCall } from '../test/spy';
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { ENGLISH, renderWithI18n } from '../test/render';
import { SERVER_COUNTS } from '../test/counts';
import { statusRevision } from '../lib/status.svelte';
import { ApiError, api } from '../api/client';
import type { AuthMode, MetadataProviders } from '../api/types';
import Settings from './Settings.svelte';
import Sources from './Sources.svelte';
import { onboarding, publishOnboarding } from '../lib/onboarding.svelte';
import { interceptLinks, navigate, router } from '../lib/router.svelte';
import { unloading } from '../test/leaving';
import { onboardingStatus } from '../test/fixtures';
import { withBase } from '../test/base';
import { answerConfirmation } from '../test/confirm';
import { captureDownloads } from '../test/downloads';
import { publishAccess } from '../lib/access.svelte';

// The proof a key needs has its own tests (`lib/proof.test.ts`): here it is
// given, as the dialog would hand it back.
vi.mock('../lib/proof.svelte', () => ({
  withProof: <T>(send: (proof: object) => Promise<T>) => send({ current_key: 'the-key-proven' }),
}));

/** The two screens built on the settings editor: Settings and the metadata sources. */

const LIVE_MODE =
  'Global dry-run is OFF: applying a decision will write to Radarr/Sonarr immediately.';
const AUTO_APPLY_ARMED =
  'Automatic application is armed: a routing decision for a title with no files yet is written to Radarr/Sonarr as soon as it is made, without confirmation.';
const SAVES_SOURCES =
  'Saves the source list and its keys, the weekly anime identifier table, and Cache and coverage. The other cards act at once.';
const API_KEY_HELP =
  'Routarr generates one at first start unless ROUTARR_API_KEY is set. Scripts send it as X-Api-Key. This browser sent it once to open a session, and keeps no copy.';

const APIKEY_MODE: AuthMode = {
  mode: 'apikey',
  api_key_configured: true,
  api_key_pinned: false,
};

type ProviderOverride = { configured?: boolean; order?: string[] };

/** The sources the server lists, TMDB answering or not. */
const catalogue = (provider: ProviderOverride): MetadataProviders => ({
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
      website: null,
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
      website: 'https://www.themoviedb.org',
    },
  ],
  order: provider.order ?? ['arr'],
});

function mount(
  settings: Record<string, string | boolean>,
  auth: AuthMode = APIKEY_MODE,
  provider: ProviderOverride = {},
  page: typeof Settings = Settings,
  direction: 'ltr' | 'rtl' = 'ltr',
) {
  vi.spyOn(api, 'authMode').mockResolvedValue(auth);
  vi.spyOn(api, 'getSettings').mockResolvedValue(settings);
  vi.spyOn(api, 'getCategories').mockResolvedValue([]);
  vi.spyOn(api, 'getLanguages').mockResolvedValue({
    default: 'en',
    languages: [
      { code: 'en', name: 'English', completion: 100, direction: 'ltr' },
      { code: 'nb_NO', name: 'Norsk bokmål', completion: 100, direction: 'ltr' },
    ],
  });
  // The backups card loads on mount, and unmocked it would reach the network.
  vi.spyOn(api, 'listBackups').mockResolvedValue({
    backups: [],
    retention_count: 7,
    copy: null,
    pending_restore: null,
    replaced_database: null,
  });
  // So does the sessions card, in every mode a browser signs in.
  vi.spyOn(api, 'getSessions').mockResolvedValue([]);
  // The signing card loads with the Notifications section, and its failure would
  // pass through every test of that section unseen.
  vi.spyOn(api, 'webhookSigning').mockResolvedValue({ signed: false, since: null, readable: true });
  vi.spyOn(api, 'getSources').mockResolvedValue(catalogue(provider));
  return renderWithI18n(page, { direction });
}

/** The metadata sources screen, with the same stand-ins as Settings. */
const mountSources = (
  settings: Record<string, string | boolean>,
  provider: ProviderOverride = {},
) => mount(settings, APIKEY_MODE, provider, Sources);

/** Open a settings section, the way a user reaches it. */
const openSection = async (name: string) =>
  fireEvent.click(await screen.findByRole('tab', { name }));

/** Click Save and hand back what went to the server. */
async function save(): Promise<Record<string, string>> {
  const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(undefined as never);
  vi.spyOn(api, 'getLocalization').mockResolvedValue({
    language: 'en',
    direction: 'ltr',
    strings: { ...ENGLISH },
    counts: [...SERVER_COUNTS],
    theme: 'dark',
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
    await screen.findByRole('tab', { name: 'Guardrails' });
    expect(screen.queryByText(AUTO_APPLY_ARMED)).toBeNull();
  });

  it('stays quiet while automatic application is off', async () => {
    mount({ global_dry_run: 'false', auto_apply_enabled: 'false' });

    await screen.findByText(LIVE_MODE);
    expect(screen.queryByText(AUTO_APPLY_ARMED)).toBeNull();
  });

  it('warns for the combination', async () => {
    mount({ global_dry_run: 'false', auto_apply_enabled: 'true' });

    expect(await screen.findByText(AUTO_APPLY_ARMED)).toBeTruthy();
    expect(screen.getByText(LIVE_MODE)).toBeTruthy();
  });

  it('stays off the sources screen', async () => {
    mountSources({ global_dry_run: 'false', auto_apply_enabled: 'true' });

    await screen.findByText('Active, in priority order');
    expect(screen.queryByText(LIVE_MODE)).toBeNull();
    expect(screen.queryByText(AUTO_APPLY_ARMED)).toBeNull();
  });
});

describe('the save bar', () => {
  it('is absent until something is edited', async () => {
    mount({ global_dry_run: 'true' });

    await screen.findByRole('tab', { name: 'Guardrails' });
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
  });

  it('appears with a count once a field changes, and says what it covers', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');

    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));

    expect(await screen.findByText('Unsaved changes: 1')).toBeTruthy();
    expect(screen.getByText('Every section is saved together')).toBeTruthy();
  });

  /** A field sent unchanged would write back what this screen read, over a save made since. */
  it('sends the fields changed, and only those', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');

    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    expect(await save()).toEqual({ global_dry_run: 'false' });
  });

  it('saves the sources alone, from a screen with no section strip', async () => {
    mountSources({ metadata_providers: 'arr' });

    await fireEvent.input(await screen.findByLabelText('Metadata cache TTL'), {
      target: { value: '14' },
    });
    expect(screen.queryByRole('tab')).toBeNull();
    expect(screen.queryByText('Every section is saved together')).toBeNull();
    expect(await save()).toEqual({ metadata_cache_ttl_days: '14' });
  });

  it('tells the shell its counters are out of date', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));

    const before = statusRevision();
    await save();

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  it('asks before leaving with unsaved changes, and stays on Cancel', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    await screen.findByText('Unsaved changes: 1');

    navigate('/rules');
    expect(await answerConfirmation(null)).toBe(
      'Leave the settings without saving? Unsaved changes: 1',
    );

    expect(router.path).not.toBe('/rules');
    expect(screen.getByText('Unsaved changes: 1')).toBeTruthy();
  });

  it('leaves once the reader agrees to drop the changes', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    await screen.findByText('Unsaved changes: 1');

    navigate('/rules');
    await answerConfirmation();

    expect(router.path).toBe('/rules');
  });

  it('leaves without a question when nothing is pending', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');

    navigate('/rules');

    await waitFor(() => expect(router.path).toBe('/rules'));
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('holds the tab open while changes are pending, and only then', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    expect(unloading()).toBe(false);

    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    await screen.findByText('Unsaved changes: 1');

    expect(unloading()).toBe(true);
  });

  it('puts the draft back when the change is discarded', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');

    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    await screen.findByText('Unsaved changes: 1');
    await fireEvent.click(screen.getByRole('button', { name: 'Discard' }));

    await waitFor(() => expect(screen.queryByText('Unsaved changes: 1')).toBeNull());
  });

  it('hands the focus to the open section once a save takes the bar away', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    (await screen.findByRole('button', { name: 'Save' })).focus();

    await save();

    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('tabpanel')));
  });

  it('hands the focus to the open section once a discard takes the bar away', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    const discard = await screen.findByRole('button', { name: 'Discard' });
    discard.focus();

    await fireEvent.click(discard);

    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('tabpanel')));
  });

  it('leaves the focus in the field a save was sent from', async () => {
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(undefined as never);
    vi.spyOn(api, 'getLocalization').mockResolvedValue({
      language: 'en',
      direction: 'ltr',
      strings: { ...ENGLISH },
      counts: [...SERVER_COUNTS],
      theme: 'dark',
    });
    mount({ batch_limit: '50' });
    await openSection('Guardrails');
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
  const noTable = () =>
    vi.spyOn(api, 'getAnimeIds').mockResolvedValue({
      loaded_at: null,
      entries: null,
      origin: null,
      address: 'https://example.org/anime-ids.json',
    });

  it('schedules the anime identifier table in its own card, saved with the rest', async () => {
    noTable();
    mountSources({ metadata_providers: 'arr' });

    const title = await screen.findByRole('heading', { name: 'Anime identifiers' });
    const card = title.closest('.card') as HTMLElement;
    const weekly = within(card).getByLabelText('Anime identifiers every week');
    expect(weekly.closest('.card')).toBe(card);
    await userEvent.click(weekly);

    expect(screen.getByText('Unsaved changes: 1')).toBeTruthy();
    expect(screen.getByText(SAVES_SOURCES)).toBeTruthy();
    expect(await save()).toEqual({ anime_ids_weekly: 'true' });
  });

  it('orders its cards from the sources to their credits, the table beside its sources', async () => {
    noTable();
    mountSources({ metadata_providers: 'arr' });

    await screen.findByRole('heading', { name: 'Anime identifiers' });
    expect(
      screen.getAllByRole('heading', { level: 2 }).map((heading) => heading.textContent?.trim()),
    ).toEqual([
      'Metadata sources',
      'Anime identifiers',
      'Cache and coverage',
      'Read the answers again',
      'Data sources',
    ]);
  });

  it('offers the credential in the row of the source that needs it', async () => {
    mountSources({ metadata_providers: 'arr' });

    const field = await screen.findByLabelText('TMDB');
    expect(field.getAttribute('id')).toBe('setting-tmdb_api_key');
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

    await userEvent.type(screen.getByLabelText('TMDB'), 'a-key');
    expect(enable.disabled).toBe(false);
  });

  it('does not delete a stored key when an unrelated setting is saved', async () => {
    mountSources(
      { metadata_cache_ttl_days: '30', metadata_providers: 'arr,tmdb' },
      { configured: true, order: ['arr', 'tmdb'] },
    );

    await fireEvent.input(await screen.findByLabelText('Metadata cache TTL'), {
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
   * "Blank leaves it alone" holds for a secret only. Applied to the source list, it would drop
   * an emptied list, say "Settings saved" and change nothing. Sent, the list is refused, which
   * tells the operator it is invalid.
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

  /** Stored by a save of anything, the order would no longer follow the server's default. */
  it('leaves the order the server resolved unstored when none is stored', async () => {
    mountSources({ metadata_cache_ttl_days: '30' }, { configured: true, order: ['tmdb', 'arr'] });

    await fireEvent.input(await screen.findByLabelText('Metadata cache TTL'), {
      target: { value: '14' },
    });
    expect(await save()).toEqual({ metadata_cache_ttl_days: '14' });
  });

  /** The server refuses a list that repeats a source, so sent back it would hold back the save. */
  it('reads a stored list without its repeated source, and an unrelated save leaves it', async () => {
    mountSources(
      { metadata_cache_ttl_days: '30', metadata_providers: 'tmdb,arr,tmdb' },
      { configured: true, order: ['tmdb', 'arr'] },
    );
    expect(await screen.findAllByRole('button', { name: 'Disable – TMDB' })).toHaveLength(1);

    await fireEvent.input(screen.getByLabelText('Metadata cache TTL'), {
      target: { value: '14' },
    });
    expect(await save()).toEqual({ metadata_cache_ttl_days: '14' });
  });

  it('separates what is active from what is switched off', async () => {
    mountSources({ metadata_providers: 'arr' });

    expect(await screen.findByText('Active, in priority order')).toBeTruthy();
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

  it('says an address is stored, and leaves it alone when saved blank', async () => {
    mount({ notification_webhook_url_configured: true });
    await openSection('Notifications');

    const field = await screen.findByLabelText('Notification webhook');
    expect(field.getAttribute('type')).toBe('password');
    expect(field.getAttribute('placeholder')).toBe('A value is stored. Type to replace it');

    await userEvent.selectOptions(screen.getByLabelText('Notification format'), 'ntfy');
    const payload = await save();
    expect(payload).not.toHaveProperty('notification_webhook_url');
    expect(payload.notification_format).toBe('ntfy');
  });

  it('removes a stored address at the next save', async () => {
    mount({ notification_webhook_url_configured: true });
    await openSection('Notifications');

    await fireEvent.click(
      await screen.findByRole('button', { name: 'Remove – Notification webhook' }),
    );

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

  it('stops marking the source as waiting for its key once a saved key lets it answer', async () => {
    mountSources({ metadata_providers: 'arr,tmdb' }, { order: ['arr', 'tmdb'] });
    expect(await screen.findByText('Waiting for its key', { selector: '.badge' })).toBeTruthy();

    await userEvent.type(screen.getByLabelText('TMDB'), 'a-key');
    vi.mocked(api.getSettings).mockResolvedValue({
      metadata_providers: 'arr,tmdb',
      tmdb_api_key_configured: true,
    });
    vi.mocked(api.getSources).mockResolvedValue(
      catalogue({ configured: true, order: ['arr', 'tmdb'] }),
    );
    await save();

    await waitFor(() =>
      expect(screen.queryByText('Waiting for its key', { selector: '.badge' })).toBeNull(),
    );
    expect(screen.getByRole('button', { name: 'Remove – TMDB' })).toBeTruthy();
  });

  it('marks the source as waiting for its key once a removed key leaves it nothing', async () => {
    mountSources(
      { metadata_providers: 'arr,tmdb', tmdb_api_key_configured: true },
      { configured: true, order: ['arr', 'tmdb'] },
    );
    await fireEvent.click(await screen.findByRole('button', { name: 'Remove – TMDB' }));
    expect(screen.queryByText('Waiting for its key', { selector: '.badge' })).toBeNull();

    vi.mocked(api.getSettings).mockResolvedValue({ metadata_providers: 'arr,tmdb' });
    vi.mocked(api.getSources).mockResolvedValue(
      catalogue({ configured: false, order: ['arr', 'tmdb'] }),
    );
    await save();

    expect(await screen.findByText('Waiting for its key', { selector: '.badge' })).toBeTruthy();
  });
});

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
  const card = async (title: string) =>
    (await screen.findByRole('heading', { name: title })).closest('.card') as HTMLElement;

  it('gathers the backups, their schedule and their encryption in one card', async () => {
    mount({ backup_enabled: 'true', backup_interval_hours: '24', backup_retention_count: '7' });
    await openSection('Maintenance');

    const backups = await card('Backups');
    expect(within(backups).getByRole('button', { name: 'Back up now' })).toBeTruthy();
    for (const label of ['Scheduled backups', 'Backup interval', 'Backups kept']) {
      expect(within(backups).getByLabelText(label)).toBeTruthy();
    }
    const sealed = within(backups).getByText('Backup encryption', { selector: 'summary *' });
    expect(sealed.closest('summary')).toHaveTextContent('Not encrypted');
    expect((sealed.closest('details') as HTMLDetailsElement).open).toBe(false);
    expect(screen.queryByRole('heading', { name: 'Backup encryption' })).toBeNull();

    await fireEvent.input(within(backups).getByLabelText('Backups kept'), {
      target: { value: '5' },
    });
    expect(await save()).toEqual({ backup_retention_count: '5' });
  });

  /** Purge deletes what retention lets go, so it sits with retention and reads as a deletion. */
  it('sets the retention apart with its purge, and the configuration file apart again', async () => {
    mount({});
    await openSection('Maintenance');

    const retention = await card('Retention');
    expect(within(retention).getByLabelText('Decision retention')).toBeTruthy();
    expect(within(retention).getByLabelText('Log retention')).toBeTruthy();
    expect(within(retention).getByRole('button', { name: 'Purge now' })).toHaveClass('btn-danger');
    expect(within(retention).queryByRole('button', { name: 'Export configuration' })).toBeNull();
    const file = await card('Configuration file');
    expect(within(file).getByRole('button', { name: 'Export configuration' })).toBeTruthy();
    expect(within(file).getByRole('button', { name: 'Import configuration' })).toBeTruthy();
    expect(within(file).getByText(/never contains your Arr API keys/)).toBeTruthy();
  });

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
    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /** A purge of a long history runs long, and the screen gone has nobody to tell. */
  it('stops following the purge once the screen is gone', async () => {
    const purge = vi.spyOn(api, 'purge').mockReturnValue(new Promise(() => {}));
    const { unmount } = mount({});
    await openSection('Maintenance');
    await fireEvent.click(await screen.findByRole('button', { name: /purge/i }));
    await answerConfirmation();
    await waitFor(() => expect(purge).toHaveBeenCalledTimes(1));

    unmount();

    expect(nthCall(purge)[0]?.signal?.aborted).toBe(true);
  });
});

describe('the section strip', () => {
  it('keeps the page address when switching sections under a mount point', async () => {
    withBase('/routarr/');
    window.history.replaceState({}, '', '/routarr/settings');
    mount({});

    await openSection('Guardrails');

    expect(window.location.pathname).toBe('/routarr/settings');
    expect(window.location.hash).toBe('#guardrails');
  });

  it('opens the section named in the URL', async () => {
    window.history.replaceState({}, '', '/settings#notifications');
    mount({});

    const tab = await screen.findByRole('tab', { name: 'Notifications' });
    expect(tab.getAttribute('aria-selected')).toBe('true');
  });

  /** The sources are a screen of their own, so `#metadata` opens nothing here. */
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
      expect(screen.getByRole('tab', { name: 'Security' }).getAttribute('aria-selected')).toBe(
        'true',
      ),
    );
  });

  it('marks each language of the picker with its own language', async () => {
    mount({});

    const option = await screen.findByRole('option', { name: 'Norsk bokmål' });
    expect(option).toHaveAttribute('lang', 'nb-NO');
  });

  it('follows the writing direction with the arrow keys', async () => {
    mount({}, APIKEY_MODE, {}, Settings, 'rtl');

    const general = await screen.findByRole('tab', { name: 'General' });
    await fireEvent.keyDown(general, { key: 'ArrowLeft' });

    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Security' }).getAttribute('aria-selected')).toBe(
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

    await fireEvent.click(await screen.findByRole('button', { name: 'Import configuration' }));
    expect(pick).toHaveBeenCalledTimes(1);
  });

  /** What the summary says of `BUNDLE`. */
  const RESTORED =
    'Restored. Settings: 3, categories: 1, instances: 0, mappings: 0, rules: 0, exceptions: 0, rule tests: 0.';

  const BUNDLE = {
    settings: 3,
    categories: 1,
    instances: 0,
    root_folders: 0,
    overrides: 0,
    rules: 0,
    rule_tests: 0,
    skipped: [],
    needs_key: [],
    adjusted: [],
  };

  /**
   * As the picker hands a file over: to the input, with the focus left where
   * it was. Import opens the picker with `click()`, which focuses nothing, and
   * an upload that clicked the input itself would put the focus on it.
   */
  async function importFile(contents: object) {
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File([JSON.stringify(contents)], 'routarr-config.json', {
      type: 'application/json',
    });
    await fireEvent.change(input, { target: { files: [file] } });
  }

  it('asks whether the rules of a bundle replace the ones in place', async () => {
    const importConfig = vi.spyOn(api, 'importConfig').mockResolvedValue({ ...BUNDLE, rules: 1 });
    mount({});
    await openSection('Maintenance');
    const withRules = { version: 1, rules: [{ name: 'Anime' }] };

    await importFile(withRules);
    expect(await answerConfirmation(null)).toBe(
      'Replace every existing rule with the ones in this file?',
    );
    await importFile(withRules);
    await answerConfirmation('replace');
    await waitFor(() => expect(importConfig).toHaveBeenCalledTimes(1));
    expect(importConfig).toHaveBeenLastCalledWith(withRules, true);

    await importFile({ version: 1 });
    await waitFor(() => expect(importConfig).toHaveBeenCalledTimes(2));
    expect(importConfig).toHaveBeenLastCalledWith({ version: 1 }, false);
  });

  it('says the summary where it is announced, and leaves the focus on Import', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({});
    await openSection('Maintenance');
    const regions = screen.getAllByRole('status');
    const button = screen.getByRole('button', { name: 'Import configuration' });
    button.focus();

    await importFile({ version: 1 });

    const said = await waitFor(() => {
      const region = screen
        .getAllByRole('status')
        .find((node) => node.textContent?.includes(RESTORED));
      expect(region).toBeDefined();
      return region;
    });
    await waitFor(() => expect(api.getSettings).toHaveBeenCalledTimes(2));
    expect(regions).toContain(said);
    expect(document.activeElement).toBe(button);
  });

  it('says a file that is not JSON in the dictionary, and sends nothing', async () => {
    const importConfig = vi.spyOn(api, 'importConfig');
    mount({});
    await openSection('Maintenance');
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File(['PK\u0003\u0004'], 'routarr-backup.zip', { type: 'application/zip' });

    await fireEvent.change(input, { target: { files: [file] } });

    expect(await screen.findByText('That file is not valid JSON')).toBeInTheDocument();
    expect(importConfig).not.toHaveBeenCalled();
  });

  it('re-reads the settings it just overwrote', async () => {
    const getSettings = vi.spyOn(api, 'getSettings');
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ ui_theme: 'dark' });
    await openSection('Maintenance');

    const before = getSettings.mock.calls.length;
    await importFile({ version: 1 });

    await waitFor(() => expect(getSettings.mock.calls.length).toBeGreaterThan(before));
  });

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

  it('replaces an unsaved edit with the values it wrote', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ global_dry_run: 'true', batch_limit: '50' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    await openSection('Maintenance');
    vi.mocked(api.getSettings).mockResolvedValue({ global_dry_run: 'true', batch_limit: '25' });

    await importFile({ version: 1 });

    await screen.findByText(RESTORED);
    await openSection('Guardrails');
    await waitFor(() =>
      expect((screen.getByLabelText('Batch limit') as HTMLInputElement).value).toBe('25'),
    );
    expect(screen.getByLabelText('Global dry-run')).toBeChecked();
    expect(screen.queryByText('Unsaved changes: 1')).toBeNull();
  });

  it('drops an edit made before it even when the read after it fails', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue(BUNDLE);
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    await openSection('Maintenance');
    vi.mocked(api.getSettings)
      .mockRejectedValueOnce(new ApiError('The settings could not be read', 409, 'conflict'))
      .mockResolvedValue({ global_dry_run: 'true' });

    await importFile({ version: 1 });
    await screen.findByText('The settings could not be read');
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(screen.queryByText('The settings could not be read')).toBeNull());
    await openSection('Guardrails');
    expect(screen.getByLabelText('Global dry-run')).toBeChecked();
    expect(screen.queryByText('Unsaved changes: 1')).toBeNull();
  });

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
      'Restored. Settings: 3, categories: 1, instances: 2, mappings: 0, rules: 0, exceptions: 0, rule tests: 0. Instances waiting for their API key: Radarr, Sonarr',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('reports what it could not restore, until the next import', async () => {
    const importConfig = vi
      .spyOn(api, 'importConfig')
      .mockResolvedValueOnce({ ...BUNDLE, skipped: ['instance "Radarr": unknown type'] })
      .mockResolvedValueOnce(BUNDLE);
    mount({});
    await openSection('Maintenance');

    await importFile({ version: 1 });

    const summary = await screen.findByText(`${RESTORED} Not restored: 1`);
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText('instance "Radarr": unknown type')).toBeTruthy();

    await importFile({ version: 1 });

    await waitFor(() => expect(importConfig).toHaveBeenCalledTimes(2));
    await screen.findByText(RESTORED);
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
      rule_tests: 0,
      skipped: ['setting "colour": unknown'],
      needs_key: [],
      adjusted: [],
    });
    mount({});
    await openSection('Maintenance');

    await importFile({ version: 1 });

    const summary = await screen.findByText(
      'Restored. Settings: 0, categories: 0, instances: 0, mappings: 0, rules: 0, exceptions: 0, rule tests: 0. Not restored: 1',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-danger')).toBe(true);
  });

  /** A bundle carries the rule tests, and one restoring them alone restored something. */
  it('counts the rule tests an import restored', async () => {
    vi.spyOn(api, 'importConfig').mockResolvedValue({
      ...BUNDLE,
      settings: 0,
      categories: 0,
      rule_tests: 2,
      skipped: [
        "Rule test 'Totoro': no instance here is named 'Radarr', the one that held its title.",
      ],
    });
    mount({});
    await openSection('Maintenance');

    await importFile({ version: 2 });

    const summary = await screen.findByText(
      'Restored. Settings: 0, categories: 0, instances: 0, mappings: 0, rules: 0, exceptions: 0, rule tests: 2. Not restored: 1',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
  });
});

describe('the API key card', () => {
  // Who may come in is the Security section's.
  beforeEach(() => window.history.replaceState({}, '', '/settings#security'));
  afterEach(() => window.history.replaceState({}, '', '/settings'));

  it('is offered in the mode whose only credential it is', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    expect(await screen.findByRole('heading', { name: 'Routarr API key' })).toBeInTheDocument();
    expect(screen.getByText(API_KEY_HELP)).toBeInTheDocument();
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
    expect(
      screen.getByText(
        'Sign-in uses your session. This key is for clients that cannot hold one, such as scripts and monitoring, which send it as X-Api-Key.',
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(API_KEY_HELP)).toBeNull();
  });

  /**
   * Something to create: this is how a script gets a credential without being
   * handed the password.
   */
  it('offers to mint one in a session mode that has none', async () => {
    mount({}, { mode: 'oidc', api_key_configured: false, api_key_pinned: false });

    expect(await screen.findByRole('button', { name: 'Create a key' })).toBeInTheDocument();
  });

  it('offers no button when the environment pins the key, and says why', async () => {
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: true });

    expect(
      await screen.findByText('ROUTARR_API_KEY sets this key, so it cannot be changed here.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Regenerate' })).toBeNull();
  });

  it('shows what it just minted, once, and keeps no copy', async () => {
    const rotate = vi.spyOn(api, 'rotateApiKey').mockResolvedValue({ api_key: 'the-new-one' });
    mount({}, { mode: 'apikey', api_key_configured: true, api_key_pinned: false });

    await fireEvent.click(await screen.findByRole('button', { name: 'Regenerate' }));
    // The question names what breaks, not just what is about to happen.
    expect(await answerConfirmation()).toBe(
      'Regenerate the API key? Every client still using the current one stops working until it is updated.',
    );

    await waitFor(() => expect(rotate).toHaveBeenCalledWith({ current_key: 'the-key-proven' }));
    expect(await screen.findByText('the-new-one')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy – API key' })).toBeInTheDocument();
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
    expect(await answerConfirmation(null)).toBe(
      'Remove the API key? Clients using it stop working immediately.',
    );

    expect(remove).not.toHaveBeenCalled();
  });

  it('removes the key once confirmed, with the proof sent', async () => {
    const remove = vi.spyOn(api, 'deleteApiKey').mockResolvedValue(undefined as never);
    mount({}, { mode: 'forms', api_key_configured: true, api_key_pinned: false });
    vi.mocked(api.authMode).mockResolvedValue({
      mode: 'forms',
      api_key_configured: false,
      api_key_pinned: false,
    });

    await fireEvent.click(await screen.findByRole('button', { name: 'Remove the key' }));
    await answerConfirmation();

    await waitFor(() => expect(remove).toHaveBeenCalledWith({ current_key: 'the-key-proven' }));
    // Its button goes with the key, and the one that makes a key takes the focus.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Create a key' })),
    );
  });
});

describe('the shape of a field', () => {
  it('draws an on/off setting as a box to tick, its help under its name', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');

    const box = await screen.findByRole('checkbox', { name: 'Global dry-run' });
    expect(box).toBeChecked();
    expect(box).toHaveAccessibleDescription(expect.stringMatching(/^While on, Routarr/));

    await userEvent.click(box);
    expect(box).not.toBeChecked();
    expect(await save()).toEqual({ global_dry_run: 'false' });
  });

  /** The width hints at the length of the value, and the unit leaves the label for it. */
  it('draws a number as a short field, its unit after it', async () => {
    mount({ backup_interval_hours: '24' });
    await openSection('Maintenance');

    const field = await screen.findByLabelText('Backup interval');
    expect(field).toHaveClass('max-w-12ch');
    expect(field).toHaveAccessibleDescription(expect.stringMatching(/^hours /));
    expect(screen.getByLabelText('Backups kept')).toHaveAccessibleDescription(
      expect.stringMatching(/^Past this count/),
    );
  });
});

describe('a number outside its bounds', () => {
  it('disables Save and marks the field, until it is back in range', async () => {
    mount({ batch_limit: '50' });
    await openSection('Guardrails');

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
    await openSection('Guardrails');
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

  it('marks the tab that holds a value outside its bounds', async () => {
    mount({ batch_limit: '50' });
    await openSection('Guardrails');
    await fireEvent.input(await screen.findByLabelText('Batch limit'), {
      target: { value: '0' },
    });

    await openSection('General');

    expect(
      screen.getByRole('tab', { name: 'Guardrails: a value is outside its bounds' }),
    ).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'General' })).toBeTruthy();
  });
});

describe('a refused save', () => {
  it('offers no retry, and keeps every edit', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
    vi.spyOn(api, 'updateSettings').mockRejectedValue(
      new ApiError('The batch limit is out of range', 400, 'bad_request'),
    );

    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(await screen.findByText('The batch limit is out of range')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
    expect(screen.getByLabelText('Global dry-run')).not.toBeChecked();
    expect(screen.getByText('Unsaved changes: 1')).toBeTruthy();
  });

  it('sends the next save once the refused one is answered', async () => {
    mount({ global_dry_run: 'true' });
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));
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

  it('holds Save while the settings cannot be read', async () => {
    mount({ global_dry_run: 'true' });
    vi.mocked(api.getSettings)
      .mockReset()
      .mockRejectedValueOnce(new ApiError('The settings could not be read', 409, 'conflict'))
      .mockResolvedValue({ global_dry_run: 'true' });
    cleanup();
    renderWithI18n(Settings);
    await screen.findByText('The settings could not be read');
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));

    expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole('button', { name: 'Dismiss' })).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() =>
      expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
  });

  it('keeps an edit across a reload, over the values it reads', async () => {
    mount({ global_dry_run: 'true', batch_limit: '25' });
    vi.mocked(api.getSettings)
      .mockReset()
      .mockRejectedValueOnce(new ApiError('The settings could not be read', 409, 'conflict'))
      .mockResolvedValue({ global_dry_run: 'true', batch_limit: '25' });
    cleanup();
    renderWithI18n(Settings);
    await screen.findByText('The settings could not be read');
    await openSection('Guardrails');
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Global dry-run' }));

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.queryByText('The settings could not be read')).toBeNull());

    expect(screen.getByLabelText('Global dry-run')).not.toBeChecked();
    expect(await save()).toEqual({ global_dry_run: 'false' });
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

  it('follows the guide from the sources screen to the guardrails', async () => {
    const stop = interceptLinks();
    try {
      // The sources step turns done on this visit, as the shell reads it again.
      publishOnboarding(onboardingStatus(['instance', 'categories', 'rule', 'simulation']));
      publishOnboarding(
        onboardingStatus(['instance', 'categories', 'metadata', 'rule', 'simulation']),
      );
      window.history.replaceState({}, '', '/sources');
      mountSources({});

      await fireEvent.click(await screen.findByRole('link', { name: 'Open the guardrails' }));

      await waitFor(() => expect(window.location.pathname).toBe('/settings'));
      expect(window.location.hash).toBe('#guardrails');
    } finally {
      stop();
    }
  });

  it('opens the section a link names without leaving the screen', async () => {
    window.history.replaceState({}, '', '/settings#general');
    mount({});
    await screen.findByRole('tab', { name: 'General' });

    navigate('/settings#guardrails');

    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Guardrails' })).toHaveAttribute(
        'aria-selected',
        'true',
      ),
    );
  });

  it('opens General on the language, the guide coming after the settings', async () => {
    window.history.replaceState({}, '', '/settings');
    mount({});

    const language = await screen.findByLabelText('Language');
    const guide = screen.getByRole('button', { name: 'Show the guide' });
    expect(language.compareDocumentPosition(guide) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
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

describe('for a reader', () => {
  /** The sessions are the owner's to read and end: a reader is refused both. */
  it('shows the owner the sessions, and a reader no sessions card', async () => {
    mount({});
    await openSection('Security');
    expect(await screen.findByRole('heading', { name: /^Sessions/ })).toBeInTheDocument();
    cleanup();

    publishAccess(true);
    mount({});
    await openSection('Security');

    await screen.findByRole('tab', { name: 'Security', selected: true });
    expect(screen.queryByRole('heading', { name: /^Sessions/ })).toBeNull();
    expect(api.getSessions).toHaveBeenCalledTimes(1);
  });
});
