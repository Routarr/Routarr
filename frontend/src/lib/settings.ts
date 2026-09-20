/**
 * What the Settings screen shows, as data.
 *
 * `FIELDS` is every setting the backend accepts, with the dictionary keys for
 * its caption and its help text and the fallback used when the server has no
 * value. `SECTIONS` groups them into the tab strip. Kept apart from the markup
 * because a test asserts the grouping covers `FIELDS` exactly once: a field in
 * neither list would be unreachable and saved with its fallback, silently.
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
    | 'theme'
    /// Sealed by the backend, never returned: the field renders empty and a
    /// companion `<key>_configured` boolean says whether one is stored.
    | 'secret';
  fallback: string;
  /**
   * The interval a `number` accepts — the bounds `api/settings.rs` enforces,
   * stated here so the field refuses a value before a save instead of after
   * it, in a refusal naming a key in a tab the operator never opened. `null`
   * is no upper bound: a retention has none.
   */
  range?: [number, number | null];
}

/**
 * Which setting holds a metadata source's credential.
 *
 * Stated here because two screens need the same answer: the source list
 * renders the field inside the row it unlocks, and the settings form skips
 * those three keys in its own loop so they are not asked for twice. Written
 * once in each place instead, adding a source removes its field from one and
 * adds it to neither.
 *
 * Not derived from `kind === 'secret'`: the application's own API key is a
 * secret too, and it belongs on its own card.
 */
export const SOURCE_KEY_SETTING: Record<string, string> = {
  tmdb: 'tmdb_api_key',
  omdb: 'omdb_api_key',
  tvdb: 'tvdb_api_key',
};

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
    kind: 'theme',
    fallback: 'dark',
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
    key: 'refresh_after_move',
    labelKey: 'SettingRefreshAfterMove',
    helpKey: 'SettingRefreshAfterMoveHelp',
    kind: 'bool',
    fallback: 'true',
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
    fallback: 'arr,tmdb',
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
    kind: 'text',
    fallback: '',
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
    range: [0, null],
  },
  {
    key: 'log_retention_days',
    labelKey: 'SettingLogRetention',
    helpKey: 'SettingLogRetentionHelp',
    kind: 'number',
    fallback: '90',
    range: [0, null],
  },
];

/**
 * The settings, grouped.
 *
 * Twenty of them in one column means scrolling past automation to reach a
 * retention count. The grouping follows what a person is *doing* — setting the
 * thing up, deciding where media goes, letting it run unattended, choosing what
 * describes it, keeping it healthy — rather than the order the fields are
 * declared in.
 *
 * `keys` is the source of truth for what appears where; a field missing from
 * every section is caught by a test rather than silently unreachable.
 */
export const SECTIONS = [
  { id: 'general', labelKey: 'SettingsTabGeneral', keys: ['ui_language', 'ui_theme'] },
  {
    id: 'routing',
    labelKey: 'SettingsTabRouting',
    keys: [
      'global_dry_run',
      'default_category',
      'batch_limit',
      'confirmation_threshold',
      'refresh_after_move',
    ],
  },
  {
    id: 'automation',
    labelKey: 'SettingsTabAutomation',
    keys: [
      'auto_sync_enabled',
      'auto_simulate_enabled',
      'auto_apply_enabled',
      'scheduler_interval_minutes',
      'notification_webhook_url',
    ],
  },
  {
    id: 'metadata',
    labelKey: 'Metadata',
    keys: [
      'metadata_providers',
      'tmdb_api_key',
      'omdb_api_key',
      'tvdb_api_key',
      'metadata_cache_ttl_days',
      'certification_regions',
    ],
  },
  {
    id: 'maintenance',
    labelKey: 'SettingsTabMaintenance',
    keys: [
      'backup_enabled',
      'backup_interval_hours',
      'backup_retention_count',
      'decision_retention_days',
      'log_retention_days',
    ],
  },
] as const;

export type SectionId = (typeof SECTIONS)[number]['id'];
