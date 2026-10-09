import type { Settings, StoredSettings } from '../api/types';

/**
 * What the Settings screen shows, as data.
 *
 * `FIELDS` is every setting the screen edits, with the dictionary keys for its
 * caption and its help text and the fallback used when the server has no
 * value. The server accepts settings no field shows, such as `onboarding`,
 * which the guide writes. `SECTIONS` groups the fields into the tab strip. Kept
 * apart from the markup because a test asserts the grouping covers `FIELDS`
 * exactly once: a field in no section would be unreachable and saved with its
 * fallback, silently.
 *
 * A new setting needs its `KNOWN` entry in `backend/src/services/settings.rs`,
 * which refuses an unknown key, and a `FIELDS` entry listed in exactly one
 * `SECTIONS` group.
 */
export interface Field {
  key: string;
  /** Dictionary keys for the label and the help text. */
  labelKey: string;
  helpKey: string;
  kind:
    | 'bool'
    | 'number'
    | 'category'
    | 'text'
    | 'language'
    | 'providers'
    /** One of `choices`. */
    | 'choice'
    /**
     * Sealed by the backend, never returned: the field renders empty and a
     * companion `<key>_configured` boolean says whether one is stored.
     */
    | 'secret';
  fallback: string;
  /**
   * The interval a `number` accepts: the bounds `services/settings.rs` enforces,
   * stated here so the field refuses a value before a save instead of after
   * it, in a refusal naming a key in a tab the operator never opened.
   */
  range?: [number, number];
  /** What a `choice` offers. */
  choices?: Choice[];
}

/** A value a `choice` offers: a caption from the dictionary, or a product's name. */
export type Choice = { value: string } & ({ labelKey: string } | { name: string });

/**
 * Which setting holds a metadata source's credential.
 *
 * Stated here because two screens need the same answer: the source list
 * renders the field inside the row it unlocks, and the settings form skips
 * those three keys in its own loop so they are not asked for twice. Written
 * once in each place instead, adding a source removes its field from one and
 * adds it to neither.
 *
 * Not derived from `kind === 'secret'`: the notification webhook is sealed
 * too, and it belongs to the automation section.
 */
export const SOURCE_KEY_SETTING: Record<string, string> = {
  tmdb: 'tmdb_api_key',
  omdb: 'omdb_api_key',
  tvdb: 'tvdb_api_key',
};

const CONFIGURED = '_configured';

/**
 * `GET /settings` split in two: the values the form edits, and the sealed
 * settings that hold one, which the server leaves out and answers with a
 * `<key>_configured` boolean only.
 */
export function splitStored(stored: StoredSettings): { values: Settings; sealed: string[] } {
  const values: Settings = {};
  const sealed: string[] = [];
  for (const [key, value] of Object.entries(stored)) {
    if (typeof value === 'string') values[key] = value;
    else if (value && key.endsWith(CONFIGURED)) sealed.push(key.slice(0, -CONFIGURED.length));
  }
  return { values, sealed };
}

export const FIELDS: Field[] = [
  {
    key: 'ui_language',
    labelKey: 'UiLanguage',
    helpKey: 'UiLanguageHelp',
    kind: 'language',
    fallback: 'en',
  },
  {
    key: 'ui_theme',
    labelKey: 'SettingTheme',
    helpKey: 'SettingThemeHelp',
    kind: 'choice',
    fallback: 'dark',
    choices: [
      { value: 'dark', labelKey: 'ThemeDark' },
      { value: 'light', labelKey: 'ThemeLight' },
      { value: 'auto', labelKey: 'ThemeAuto' },
    ],
  },
  {
    key: 'global_dry_run',
    labelKey: 'SettingGlobalDryRun',
    helpKey: 'SettingGlobalDryRunHelp',
    kind: 'bool',
    fallback: 'true',
  },
  {
    key: 'default_category',
    labelKey: 'SettingDefaultCategory',
    helpKey: 'SettingDefaultCategoryHelp',
    kind: 'category',
    fallback: 'standard',
  },
  {
    key: 'batch_limit',
    labelKey: 'SettingBatchLimit',
    helpKey: 'SettingBatchLimitHelp',
    kind: 'number',
    fallback: '50',
    range: [1, 1_000],
  },
  {
    key: 'confirmation_threshold',
    labelKey: 'SettingConfirmationThreshold',
    helpKey: 'SettingConfirmationThresholdHelp',
    kind: 'number',
    fallback: '10',
    range: [1, 10_000],
  },
  {
    key: 'auto_sync_enabled',
    labelKey: 'SettingAutoSync',
    helpKey: 'SettingAutoSyncHelp',
    kind: 'bool',
    fallback: 'true',
  },
  {
    key: 'auto_simulate_enabled',
    labelKey: 'SettingAutoSimulate',
    helpKey: 'SettingAutoSimulateHelp',
    kind: 'bool',
    fallback: 'true',
  },
  {
    key: 'auto_apply_enabled',
    labelKey: 'SettingAutoApply',
    helpKey: 'SettingAutoApplyHelp',
    kind: 'bool',
    fallback: 'false',
  },
  {
    key: 'scheduler_interval_minutes',
    labelKey: 'SettingSchedulerInterval',
    helpKey: 'SettingSchedulerIntervalHelp',
    kind: 'number',
    fallback: '15',
    range: [1, 24 * 60],
  },
  {
    key: 'metadata_providers',
    labelKey: 'SettingMetadataProviders',
    helpKey: 'SettingMetadataProvidersHelp',
    kind: 'providers',
    // The screen seeds this field from the order the server resolves, so the
    // fallback only has to be a list the backend accepts.
    fallback: 'arr',
  },
  {
    // The values of `metadata::ANIME_SEARCH` in `backend/src/services/metadata.rs`.
    key: 'anime_search',
    labelKey: 'SettingAnimeSearch',
    helpKey: 'SettingAnimeSearchHelp',
    kind: 'choice',
    fallback: 'animated',
    choices: [
      { value: 'animated', labelKey: 'AnimeSearchAnimated' },
      { value: 'all', labelKey: 'AnimeSearchAll' },
    ],
  },
  // The three metadata credentials. Sealed by the backend on the way in and
  // never returned, so the field always renders empty and a companion
  // `<key>_configured` boolean says whether one is set. Settable here as well as
  // from the environment: the screen that names a missing key is the screen that
  // should be able to supply it.
  {
    key: 'tmdb_api_key',
    labelKey: 'SettingTmdbKey',
    helpKey: 'SettingTmdbKeyHelp',
    kind: 'secret',
    fallback: '',
  },
  {
    key: 'omdb_api_key',
    labelKey: 'SettingOmdbKey',
    helpKey: 'SettingOmdbKeyHelp',
    kind: 'secret',
    fallback: '',
  },
  {
    key: 'omdb_daily_requests',
    labelKey: 'SettingOmdbDailyRequests',
    helpKey: 'SettingOmdbDailyRequestsHelp',
    kind: 'number',
    fallback: '1000',
    range: [1, 1_000_000],
  },
  {
    key: 'tvdb_api_key',
    labelKey: 'SettingTvdbKey',
    helpKey: 'SettingTvdbKeyHelp',
    kind: 'secret',
    fallback: '',
  },
  {
    key: 'metadata_cache_ttl_days',
    labelKey: 'SettingMetadataTtl',
    helpKey: 'SettingMetadataTtlHelp',
    kind: 'number',
    fallback: '7',
    range: [1, 3_650],
  },
  {
    key: 'notification_webhook_url',
    labelKey: 'SettingNotificationWebhook',
    helpKey: 'SettingNotificationWebhookHelp',
    kind: 'secret',
    fallback: '',
  },
  {
    // The values of `notify::FORMATS` in `backend/src/services/notify.rs`.
    key: 'notification_format',
    labelKey: 'SettingNotificationFormat',
    helpKey: 'SettingNotificationFormatHelp',
    kind: 'choice',
    fallback: 'auto',
    choices: [
      { value: 'auto', labelKey: 'NotificationFormatAuto' },
      { value: 'json', labelKey: 'NotificationFormatJson' },
      { value: 'discord', name: 'Discord' },
      { value: 'ntfy', name: 'ntfy' },
      { value: 'gotify', name: 'Gotify' },
      { value: 'apprise', name: 'Apprise' },
    ],
  },
  {
    key: 'notify_sync_failed',
    labelKey: 'SettingNotifySyncFailed',
    helpKey: 'SettingNotifySyncFailedHelp',
    kind: 'bool',
    fallback: 'false',
  },
  {
    key: 'notify_simulation_completed',
    labelKey: 'SettingNotifySimulationCompleted',
    helpKey: 'SettingNotifySimulationCompletedHelp',
    kind: 'bool',
    fallback: 'false',
  },
  {
    key: 'notify_moves_completed',
    labelKey: 'SettingNotifyMovesCompleted',
    helpKey: 'SettingNotifyMovesCompletedHelp',
    kind: 'bool',
    fallback: 'false',
  },
  {
    key: 'certification_regions',
    labelKey: 'SettingCertificationRegions',
    helpKey: 'SettingCertificationRegionsHelp',
    kind: 'text',
    fallback: 'US',
  },
  {
    key: 'backup_enabled',
    labelKey: 'SettingBackupEnabled',
    helpKey: 'SettingBackupEnabledHelp',
    kind: 'bool',
    fallback: 'true',
  },
  {
    key: 'backup_interval_hours',
    labelKey: 'SettingBackupInterval',
    helpKey: 'SettingBackupIntervalHelp',
    kind: 'number',
    fallback: '24',
    range: [1, 24 * 7],
  },
  {
    key: 'backup_retention_count',
    labelKey: 'SettingBackupRetention',
    helpKey: 'SettingBackupRetentionHelp',
    kind: 'number',
    fallback: '7',
    range: [1, 50],
  },
  {
    key: 'decision_retention_days',
    labelKey: 'SettingDecisionRetention',
    helpKey: 'SettingDecisionRetentionHelp',
    kind: 'number',
    fallback: '30',
    range: [0, 3650],
  },
  {
    key: 'log_retention_days',
    labelKey: 'SettingLogRetention',
    helpKey: 'SettingLogRetentionHelp',
    kind: 'number',
    fallback: '90',
    range: [0, 3650],
  },
  {
    key: 'security_log_retention_days',
    labelKey: 'SettingSecurityLogRetention',
    helpKey: 'SettingSecurityLogRetentionHelp',
    kind: 'number',
    fallback: '365',
    range: [0, 3650],
  },
];

/**
 * The settings, grouped.
 *
 * Every setting in one column means scrolling past automation to reach a
 * retention count. The grouping follows what a person is *doing* (setting the
 * thing up, deciding where media goes, letting it run unattended, hearing about
 * it, choosing what describes it, keeping it healthy) rather than the order the
 * fields are declared in.
 *
 * `page` says which screen edits the section: the metadata sources have their
 * own, since the guide sends a fresh install there and their warnings name it.
 * Each screen saves its own sections and no other.
 *
 * `keys` is the source of truth for what appears where. A field missing from
 * every section is caught by a test rather than silently unreachable.
 */
export const SECTIONS = [
  {
    id: 'general',
    page: 'settings',
    labelKey: 'SettingsTabGeneral',
    keys: ['ui_language', 'ui_theme'],
  },
  {
    // Who may come in and what they did: the key, the account and the
    // sessions are drawn beside this field (`SettingsEditor`).
    id: 'security',
    page: 'settings',
    labelKey: 'SettingsTabSecurity',
    keys: ['security_log_retention_days'],
  },
  {
    id: 'guardrails',
    page: 'settings',
    labelKey: 'SettingsTabGuardrails',
    keys: ['global_dry_run', 'default_category', 'batch_limit', 'confirmation_threshold'],
  },
  {
    id: 'automation',
    page: 'settings',
    labelKey: 'SettingsTabAutomation',
    keys: [
      'auto_sync_enabled',
      'auto_simulate_enabled',
      'auto_apply_enabled',
      'scheduler_interval_minutes',
    ],
  },
  {
    id: 'notifications',
    page: 'settings',
    labelKey: 'SettingsTabNotifications',
    keys: [
      'notification_webhook_url',
      'notification_format',
      'notify_sync_failed',
      'notify_simulation_completed',
      'notify_moves_completed',
    ],
  },
  {
    id: 'maintenance',
    page: 'settings',
    labelKey: 'SettingsTabMaintenance',
    keys: [
      'backup_enabled',
      'backup_interval_hours',
      'backup_retention_count',
      'decision_retention_days',
      'log_retention_days',
    ],
  },
  {
    id: 'metadata',
    page: 'sources',
    labelKey: 'Metadata',
    keys: [
      'metadata_providers',
      'anime_search',
      'tmdb_api_key',
      'omdb_api_key',
      'omdb_daily_requests',
      'tvdb_api_key',
      'metadata_cache_ttl_days',
      'certification_regions',
    ],
  },
] as const;

export type SectionId = (typeof SECTIONS)[number]['id'];

/** A screen that edits settings: each holds the sections naming it. */
export type SettingsPage = (typeof SECTIONS)[number]['page'];
