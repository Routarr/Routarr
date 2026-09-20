import type { Decision, Health, Instance, Job, MediaListItem } from '../api/types';

let counter = 0;

/** A decision row, shaped like the backend's. */
export function decision(over: Partial<Decision> = {}): Decision {
  counter += 1;
  return {
    id: `d${counter}`,
    media_id: `m${counter}`,
    media_title: 'Akira',
    media_type: 'movie',
    instance_id: 'i1',
    instance_name: 'Radarr',
    current_root_folder: '/films',
    target_root_folder: '/anime',
    target_category: 'anime',
    matched_rule_id: 'r1',
    matched_rule_name: 'Japanese',
    is_override: false,
    reasons: [],
    alternatives: [],
    action: 'move',
    status: 'pending',
    confidence: 0.9,
    superseded: false,
    simulation_id: 's1',
    error_message: null,
    decided_at: '2026-08-27 10:00:00',
    applied_at: null,
    reverted_at: null,
    actor: null,
    subject: null,
    ...over,
  };
}

/** Wrap rows in the envelope every paginated endpoint returns. */
export function paginated<T>(
  data: T[],
  over: Partial<{ page: number; per_page: number; total: number; total_pages: number }> = {},
) {
  return {
    data,
    pagination: { page: 1, per_page: 50, total: data.length, total_pages: 1, ...over },
  };
}

/**
 * A health payload.
 *
 * Three screens read it — the dashboard, diagnostics and the top bar. A literal
 * built per test drifts: a field added to `Health` reaches whichever fixture the
 * author happened to open. One fixture, overridden per test, keeps them honest.
 */
export function health(over: Partial<Health> = {}): Health {
  return {
    status: 'ok',
    version: '0.1.0',
    database: 'connected',
    warnings: [],
    instances: [],
    metadata: {
      providers: [
        {
          id: 'arr',
          display_name: 'Radarr / Sonarr',
          needs_key: false,
          configured: true,
          connected: null,
        },
      ],
      cached_items: 0,
      media_missing_metadata: 0,
    },
    stats: {
      total_instances: 0,
      total_media: 0,
      total_movies: 0,
      total_series: 0,
      total_rules: 0,
      enabled_rules: 0,
      total_overrides: 0,
      pending_decisions: 0,
      applied_decisions: 0,
      failed_decisions: 0,
      unmapped_categories: 0,
      running_jobs: 0,
    },
    ...over,
  };
}

/** An instance row, as `/health` reports it. */
export function healthInstance(
  over: Partial<Health['instances'][number]> = {},
): Health['instances'][number] {
  return {
    id: 'i1',
    name: 'Radarr',
    instance_type: 'radarr',
    status: 'connected',
    version: '5.0.0',
    last_sync: '2026-08-27 10:00:00',
    last_sync_status: 'success',
    media_count: 120,
    mapped_root_folders: 2,
    ...over,
  };
}

export function instance(over: Partial<Instance> = {}): Instance {
  return {
    id: 'i1',
    name: 'Radarr',
    instance_type: 'radarr',
    base_url: 'http://localhost:7878',
    api_key_masked: '••••',
    api_key_encrypted: true,
    enabled: true,
    sync_interval_minutes: 15,
    last_sync_at: null,
    last_sync_attempt_at: null,
    last_sync_status: null,
    webhook_url: null,
    created_at: '2026-08-27 10:00:00',
    updated_at: '2026-08-27 10:00:00',
    ...over,
  };
}

export function job(over: Partial<Job> = {}): Job {
  return {
    id: 'j1',
    kind: 'sync',
    status: 'success',
    trigger: 'scheduled',
    instance_id: null,
    detail: null,
    progress_current: 0,
    progress_total: 0,
    error_message: null,
    started_at: '2026-08-27 10:00:00',
    finished_at: '2026-08-27 10:00:04',
    ...over,
  };
}

export function media(over: Partial<MediaListItem> = {}): MediaListItem {
  return {
    id: 'm1',
    instance_id: 'i1',
    instance_name: 'Radarr',
    arr_id: 1,
    media_type: 'movie',
    title: 'Akira',
    year: 1988,
    tmdb_id: 149,
    current_root_folder: '/data/films',
    monitored: true,
    has_files: true,
    status: 'released',
    last_synced_at: '2026-08-27 10:00:00',
    computed_category: 'anime',
    override_category: null,
    has_metadata: true,
    ...over,
  };
}
