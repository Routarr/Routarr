import type { Warning } from '../api/types';
import { t } from './i18n.svelte';
import { screenKey } from './routes';

/**
 * The screen that settles each warning code, the one its sentence names if any, or `null` where
 * the fix lies outside Routarr. `warnings.test.ts` holds it to `CODES` in `api/health.rs`.
 */
export const SETTLED_ON: Record<string, string | null> = {
  api_unauthenticated: null,
  api_external_auth: null,
  source_needs_key: '/sources',
  source_key_unlisted: '/sources',
  source_unreachable: '/sources',
  source_quota_spent: '/sources',
  source_rate_limited: '/sources',
  source_key_refused: '/sources',
  instance_unreachable: '/instances',
  instance_sync_failing: '/instances',
  unmapped_categories: '/categories',
  no_enabled_instance: '/instances',
  missing_metadata: '/sources',
  scheduler_panicked: null,
  backup_failed: '/tasks',
  backup_overdue: '/tasks',
  data_disk_low: null,
  setting_above_maximum: '/settings#maintenance',
  instance_without_mapping: '/categories',
  certification_country_outside_regions: '/sources',
  certification_country_changed: '/instances',
  auto_apply_held: '/simulation',
  arr_below_version: null,
  oidc_open_to_anyone: null,
  backup_passphrase_unreadable: '/settings#maintenance',
  rule_tests_failing: '/rule-tests?failing=1',
  webhook_never_delivered: '/instances',
  webhook_misconfigured: '/instances',
  backup_copy_failed: '/settings#maintenance',
  deprecated_api_called: '/reference',
  removal_held: '/instances',
  restore_pending: '/settings#maintenance',
};

/** Where a warning is settled and the words of the link there, or `null` where no screen is. */
export function fixOf(warning: Warning): { to: string; label: string } | null {
  const to = SETTLED_ON[warning.code];
  if (!to) return null;
  return { to, label: t('HelpFixOn', { screen: t(screenKey(to.split(/[?#]/)[0]!)) }) };
}
