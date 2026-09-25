// Shapes returned by the Routarr API. Kept in one place so a backend rename
// surfaces as a type error rather than an `undefined` in the UI.

export interface Paginated<T> {
  data: T[];
  pagination: { page: number; per_page: number; total: number; total_pages: number };
}

export type MediaType = 'movie' | 'series';
export type RuleMediaType = MediaType | 'both';
export type MatchMode = 'all' | 'any';
export type DecisionAction = 'move' | 'none' | 'skip';
export type DecisionStatus = 'pending' | 'applied' | 'failed' | 'skipped';

export interface Instance {
  id: string;
  name: string;
  instance_type: 'radarr' | 'sonarr';
  base_url: string;
  api_key_masked: string;
  api_key_encrypted: boolean;
  enabled: boolean;
  sync_interval_minutes: number;
  /// When a sync last *succeeded*. Null while none ever has.
  last_sync_at: string | null;
  /// When one was last attempted, successful or not.
  last_sync_attempt_at: string | null;
  last_sync_status: string | null;
  webhook_url: string | null;
  created_at: string;
  updated_at: string;
}

export interface RootFolder {
  id: string;
  instance_id: string;
  /** `null` for a folder Routarr declares: it has no id in the Arr. */
  arr_id: number | null;
  path: string;
  free_space: number | null;
  accessible: boolean;
  category: string | null;
  last_synced_at: string | null;
  /** When it last answered, not when it was last seen in a pass. */
  last_accessible_at: string | null;
  /** `arr` for a folder the instance reports, `declared` for one typed here. */
  origin: string;
  instance_name: string;
  instance_type: string;
}

export interface MappingConflict {
  kind: string;
  severity: 'error' | 'warning';
  instance_name: string | null;
  category: string | null;
  message: string;
}

export interface Category {
  id: string;
  name: string;
  description: string | null;
  is_default: boolean;
  display_order: number;
  created_at: string;
  rule_count: number;
  root_folder_count: number;
}

/** Discriminated by `type`; `value` shape follows the condition catalog. */
export interface Condition {
  type: string;
  value?: unknown;
}

export interface Rule {
  id: string;
  name: string;
  description: string | null;
  priority: number;
  enabled: boolean;
  media_type: RuleMediaType;
  conditions: Condition[];
  exclusions: Condition[];
  match_mode: MatchMode;
  target_category: string;
  instance_ids: string[] | null;
  created_at: string;
  updated_at: string;
}

export interface RuleDraft {
  name: string;
  description?: string | null;
  priority: number;
  enabled: boolean;
  media_type: RuleMediaType;
  conditions: Condition[];
  exclusions: Condition[];
  match_mode: MatchMode;
  target_category: string;
  instance_ids?: string[] | null;
}

export interface ConditionSpec {
  type: string;
  label: string;
  value_type: 'string' | 'string_list' | 'number' | 'number_list' | 'boolean' | 'year_range';
  needs_metadata: boolean;
  /// The metadata field it reads, if any.
  metadata_field: string | null;
  /// Whether an enabled source provides that field. A condition can be listed
  /// and unanswerable, and the builder says which.
  available: boolean;
  /// The media types this condition can ever match on. Radarr reports no
  /// `tvdbId`, no `seriesType` and no seasons, so three conditions are
  /// series-only and would never fire on a film.
  media_types: string[];
  /// The `LibraryFacets` axis its values are drawn from, empty when the library
  /// cannot enumerate them — a keyword or a title fragment is not a closed set.
  suggestions: string;
  /// How this condition's values combine — `any` or `all` — and the kind asking
  /// the same question the other way. Both empty where the media side is a
  /// single value, which cannot be asked for "all of".
  quantifier: string;
  counterpart: string;
}

export interface Localization {
  language: string;
  /// `ltr` or `rtl` for `language`, decided by the backend.
  direction: 'ltr' | 'rtl';
  strings: Record<string, string>;
}

export interface BackupFile {
  name: string;
  size_bytes: number;
  created_at: string;
}

/** What a retention purge removed. */
export interface MaintenanceReport {
  decisions_removed: number;
  logs_removed: number;
  jobs_removed: number;
  metadata_cache_removed: number;
  sessions_removed: number;
}

/** Which gate the shell must show. Readable without a session, by necessity. */
export interface AuthMode {
  mode: 'none' | 'apikey' | 'forms' | 'external' | 'oidc';
  /** Whether the server holds a key at all. False in `forms` and `oidc` until
   *  somebody sets one, which is what makes the field to paste it pointless. */
  api_key_configured: boolean;
  /** Set through `ROUTARR_API_KEY`, so it cannot be replaced from here: the new
   *  one would last until the next restart. */
  api_key_pinned: boolean;
}

/** What a restore answers with — staged, not applied until the next start. */
export interface RestoreResult {
  /** What the archive said about itself: the version and schema it was taken
   *  from, and when. The interface shows only `restart_required` today; the
   *  manifest is declared because the response really does carry it, and
   *  because it is what a "restored from a newer Routarr" message would read. */
  manifest: BackupManifest;
  restart_required: boolean;
}

/** Written into every archive so a restore can refuse what it cannot honour. */
export interface BackupManifest {
  version: string;
  schema: string;
  created_at: string;
  includes_master_key: boolean;
}

export interface BackupList {
  backups: BackupFile[];
  /// How many are kept before the oldest is pruned.
  retention_count: number;
}

export interface MetadataProvider {
  id: string;
  display_name: string;
  /// False for a source whose data arrives with the library sync.
  fetched: boolean;
  needs_key: boolean;
  /// The environment variable its credential comes from, so the interface can
  /// name it instead of saying "a key is missing".
  key_env: string | null;
  configured: boolean;
  fields: string[];
}

export interface MetadataProviders {
  providers: MetadataProvider[];
  /// Enabled sources, highest priority first; anything absent is off.
  order: string[];
}

export interface ConditionCatalog {
  match_modes: MatchMode[];
  conditions: ConditionSpec[];
}

export interface ValidationIssue {
  severity: 'error' | 'warning';
  field: string;
  message: string;
}

export interface SimulationSummary {
  total_media: number;
  moves_required: number;
  already_correct: number;
  no_category_match: number;
  skipped_unmapped: number;
  excluded_by_rule: number;
}

export interface PreviewChange {
  media_id: string;
  media_title: string;
  media_type: MediaType;
  instance_name: string | null;
  from_category: string;
  to_category: string;
  current_root_folder: string | null;
  target_root_folder: string | null;
  reasons: string[];
  confidence: number;
}

export interface RulePreview {
  issues: ValidationIssue[];
  before: SimulationSummary;
  after: SimulationSummary;
  changed: PreviewChange[];
  changed_total: number;
}

export interface AlternativeDecision {
  rule_name: string;
  category: string;
  reason: string;
  excluded_by: string | null;
  confidence: number;
}

export interface Decision {
  id: string;
  media_id: string;
  media_title: string;
  media_type: MediaType;
  instance_id: string;
  instance_name: string | null;
  current_root_folder: string | null;
  target_root_folder: string | null;
  target_category: string;
  matched_rule_id: string | null;
  matched_rule_name: string | null;
  is_override: boolean;
  reasons: string[];
  alternatives: AlternativeDecision[];
  action: DecisionAction;
  status: DecisionStatus;
  confidence: number;
  superseded: boolean;
  simulation_id: string | null;
  error_message: string | null;
  decided_at: string;
  applied_at: string | null;
  reverted_at: string | null;
  /** What caused this decision: `manual`, `schedule` or `webhook`. Null on rows
   * written before the column existed. */
  actor: string | null;
  /** Who asked. Null for the scheduler, for the modes that name nobody, and for
   * rows written before the column existed. */
  subject: string | null;
}

export interface SimulationResult extends SimulationSummary {
  /// One entry per destination this plan feeds, worst first.
  capacity: CapacityForecast[];
  simulation_id: string;
  returned: number;
  decisions: Decision[];
  overrides_applied: number;
  elapsed_ms: number;
}

export interface ApplyReport {
  requested: number;
  applied: number;
  failed: number;
  skipped: number;
  errors: { decision_id: string; media_title: string; message: string }[];
}

/** Outcome of applying a whole simulation slice by slice. */
export interface BatchApplyReport {
  candidates: number;
  applied: number;
  failed: number;
  skipped: number;
  batches_run: number;
  batches_planned: number;
  /** A slice failed and the remaining ones were abandoned. */
  stopped_early: boolean;
  errors: { decision_id: string; media_title: string; message: string }[];
}

export interface MediaListItem {
  id: string;
  instance_id: string;
  instance_name: string;
  arr_id: number;
  media_type: MediaType;
  title: string;
  year: number | null;
  tmdb_id: number | null;
  current_root_folder: string | null;
  monitored: boolean;
  has_files: boolean;
  status: string | null;
  last_synced_at: string | null;
  computed_category: string | null;
  override_category: string | null;
  has_metadata: boolean;
}

/// Every enabled source collapsed in priority order, not one provider's answer.
export interface MediaMetadata {
  genres: string[];
  keywords: string[];
  original_language: string | null;
  origin_countries: string[];
  certification: string | null;
  status: string | null;
  overview: string | null;
  poster_path: string | null;
  /// Field name -> the source that supplied it.
  field_sources: Record<string, string>;
  /// Sources that supplied at least one field, highest priority first.
  sources: string[];
}

export interface ConditionOutcome {
  kind: string;
  matched: boolean;
  expected: string;
  observed: string;
  /// Which metadata source the observed value came from, when it reads one.
  source?: string | null;
}

export interface RuleTrace {
  rule_id: string;
  rule_name: string;
  priority: number;
  category: string;
  matched: boolean;
  excluded_by: string | null;
  outcome: 'winner' | 'excluded' | 'matched_lower_priority' | 'not_matched';
  conditions: ConditionOutcome[];
}

export interface Explanation {
  media: MediaListItem & { current_path: string | null; added_at: string | null };
  metadata: MediaMetadata | null;
  override_category: string | null;
  target_category: string;
  target_root_folder: string | null;
  action: DecisionAction;
  confidence: number;
  winning_rule: string | null;
  rule_traces: RuleTrace[];
}

export interface OverrideEntry {
  id: string;
  media_id: string;
  target_category: string;
  reason: string | null;
  locked: boolean;
  created_at: string;
  media_title: string;
  media_type: MediaType;
  instance_name: string;
}

export interface Job {
  id: string;
  kind: string;
  status: 'queued' | 'running' | 'success' | 'failed';
  trigger: string;
  instance_id: string | null;
  detail: string | null;
  progress_current: number;
  progress_total: number;
  error_message: string | null;
  started_at: string;
  finished_at: string | null;
}

export interface LogEntry {
  id: string;
  decision_id: string | null;
  action: string;
  details: string | null;
  success: boolean;
  error_message: string | null;
  instance_id: string | null;
  media_id: string | null;
  media_title: string | null;
  executed_at: string;
}

export interface Status {
  version: string;
  dry_run: boolean;
  running_jobs: number;
  pending_decisions: number;
  failed_decisions: number;
  warnings: string[];
}

export interface Health {
  status: string;
  version: string;
  database: string;
  warnings: string[];
  instances: {
    id: string;
    name: string;
    instance_type: string;
    status: string;
    version: string | null;
    last_sync: string | null;
    last_sync_status: string | null;
    media_count: number;
    mapped_root_folders: number;
  }[];
  metadata: {
    providers: {
      id: string;
      display_name: string;
      needs_key: boolean;
      configured: boolean;
      connected: boolean | null;
    }[];
    cached_items: number;
    media_missing_metadata: number;
  };
  stats: {
    total_instances: number;
    total_media: number;
    total_movies: number;
    total_series: number;
    total_rules: number;
    enabled_rules: number;
    total_overrides: number;
    pending_decisions: number;
    applied_decisions: number;
    failed_decisions: number;
    unmapped_categories: number;
    running_jobs: number;
  };
}

/** What a connection probe reports. `app_name` is null for an Arr that does not say. */
export interface TestConnectionResponse {
  success: boolean;
  version: string;
  app_name: string | null;
  root_folders: number;
  inaccessible_root_folders: number;
}

export interface SyncReport {
  instance_id: string;
  instance_name: string;
  root_folders: number;
  media: number;
  removed: number;
  /** Present when this instance's sync failed. */
  error?: string;
}

export type Settings = Record<string, string>;

/** What a configuration restore managed, and what it could not. */
export interface ConfigImportReport {
  settings: number;
  categories: number;
  instances: number;
  root_folders: number;
  overrides: number;
  /** Everything not restored, and why. Never silent. */
  skipped: string[];
  /** The instances restored disabled, by name, each waiting for its API key. */
  needs_key: string[];
}

/** A pinned expectation: these inputs must keep producing this category. */
export interface RuleTest {
  id: string;
  name: string;
  media_type: string;
  expected_category: string;
  source_media_title: string | null;
  evaluated_at: string;
  created_at: string;
}

export interface RuleTestResult {
  id: string;
  name: string;
  expected_category: string;
  /// Where the engine puts it today. Null only when the snapshot stopped parsing.
  actual_category: string | null;
  passed: boolean;
  matched_rule: string | null;
  error: string | null;
}

export interface RuleTestRun {
  total: number;
  passed: number;
  failed: number;
  results: RuleTestResult[];
}

/** One destination folder, and the weight of what a plan sends to it. */
export interface CapacityForecast {
  instance_id: string;
  instance_name: string | null;
  path: string;
  /// Bytes crossing from another filesystem — the only traffic that consumes space.
  incoming_bytes: number;
  /// Bytes moving within one filesystem, where a move is a rename and costs nothing.
  same_filesystem_bytes: number;
  free_bytes: number;
  items: number;
  fits: boolean;
}

/** What the library says about one rule. */
export interface RuleHealth {
  rule_id: string;
  rule_name: string;
  priority: number;
  enabled: boolean;
  won: number;
  shadowed: number;
  vetoed: number;
  /// Set only when the rule never wins: the rule taking most of what it matched.
  shadowed_by: string | null;
  matched_nothing: boolean;
  /// Another rule with the same name *and* priority: the tie falls to the id.
  ambiguous_with: string | null;
  /// Another rule with identical conditions and target.
  duplicate_of: string | null;
}

export interface RuleHealthReport {
  total_media: number;
  rules: RuleHealth[];
}

/** One value present in the library, and how many items carry it. */
export interface Facet {
  /// What a rule stores and the engine compares — never the displayed text: a
  /// language is matched on `ja`, however it is shown.
  value: string;
  /// What to show instead of `value`, where the two differ. Absent for a genre,
  /// which is its own label.
  label?: string;
  count: number;
}

/// Values a condition may hold that the library does not define. A genre means
/// what the library says it means; a language is an ISO code from a fixed table,
/// and offering only the ones already synced would hide the rest.
export interface Vocabularies {
  original_languages: Facet[];
  origin_countries: Facet[];
}

/** What the library actually holds, per axis a condition can read. */
export interface LibraryFacets {
  total_media: number;
  vocabularies: Vocabularies;
  /// Carrying neither a genre nor an original language — invisible to every
  /// condition that reads metadata.
  without_metadata: number;
  genres: Facet[];
  original_languages: Facet[];
  origin_countries: Facet[];
  certifications: Facet[];
  tags: Facet[];
  series_types: Facet[];
  root_folders: Facet[];
}

/**
 * The axes of `LibraryFacets` that carry counted values.
 *
 * Derived rather than listed, so adding an axis to the interface above adds it
 * here too. What it buys is that indexing the payload is checked: the axis name
 * itself arrives from the backend at run time and only a test can vouch for it
 * — `every_axis_a_spec_names_is_one_the_facets_payload_answers`, in
 * `api/conditions.rs` — but everything after the narrowing is the compiler's
 * again. Widened to `Record<string, Facet[]>`, a renamed axis produced an empty
 * picker and no error anywhere.
 */
export type FacetAxis = {
  [K in keyof LibraryFacets]: LibraryFacets[K] extends Facet[] ? K : never;
}[keyof LibraryFacets];
