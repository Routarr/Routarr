// Written by hand: `scripts/check-api-types.py` pairs each with its backend struct in `PAIRS`.

export interface Paginated<T> {
  data: T[];
  pagination: { page: number; per_page: number; total: number; total_pages: number };
}

export type MediaType = 'movie' | 'series';
export type RuleMediaType = MediaType | 'both';
export type MatchMode = 'all' | 'any';
export type DecisionAction = 'move' | 'none' | 'skip';
/** `requested`: the Arr took the move and was not yet seen to finish it. */
export type DecisionStatus = 'pending' | 'requested' | 'applied' | 'failed' | 'skipped';
/** Why a run that moves titles ended before its last move. */
export type StopReason =
  'dry_run' | 'cancelled' | 'shutdown' | 'unreachable' | 'stalled' | 'refused' | 'failed';
export type LogAction = 'move' | 'revert';

export interface Instance {
  id: string;
  name: string;
  instance_type: 'radarr' | 'sonarr';
  base_url: string;
  api_key_masked: string;
  api_key_encrypted: boolean;
  /** Whether the Arr's key is stored: an imported instance waits for it. */
  api_key_set: boolean;
  enabled: boolean;
  sync_interval_minutes: number;
  /** Whether automatic application may write here, under the global switch. */
  auto_apply: boolean;
  /** When a webhook from it last arrived, null while none has since it was added or rotated. */
  last_webhook_at: string | null;
  /** The event that delivery carried, as `Test`. */
  last_webhook_event: string | null;
  /** The Arr's webhook connection as last read, null until a sync or a test reads it. */
  webhook_check: WebhookCheck | null;
  /** The switches that connection needs ticked, as the Arr's interface names them. */
  webhook_triggers: string[];
  /** How many titles the last sync kept although the Arr no longer reports them. */
  removal_held: number;
  /** When a sync last *succeeded*. Null while none ever has. */
  last_sync_at: string | null;
  /** When one was last attempted, successful or not. */
  last_sync_attempt_at: string | null;
  last_sync_status: string | null;
  /** The webhook address with its token in it, for an Arr that sends no header. */
  webhook_url: string | null;
  /** The address without the token, which goes in `X-Routarr-Token`. */
  webhook_path: string | null;
  webhook_token: string | null;
  created_at: string;
  updated_at: string;
}

/** The Arr's webhook connection to an instance, as Routarr last read it. */
export interface WebhookCheck {
  state: 'ok' | 'absent' | 'misconfigured';
  /** Null when no connection names the instance. */
  token: 'ok' | 'stale' | 'missing' | 'unchecked' | null;
  missing_triggers: string[];
  /** The method, when Routarr refuses it. */
  method: string | null;
  tags: string[];
  /** The instance a connection on this Arr names instead. */
  other_instance: string | null;
  /** What to fix, each a sentence in the interface language. */
  problems: string[];
  /** What is worth knowing and stops nothing. */
  notes: string[];
  checked_at: string;
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

/** Discriminated by `type`. The shape of `value` follows the condition catalog. */
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
  /** The template and version it was added from, as `children@1`, null when written by hand. */
  template: string | null;
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
  /** The family the builder lists it under. */
  group: 'metadata' | 'arr' | 'title' | 'folder';
  label: string;
  /** The dictionary key `label` was written from. */
  label_key: string;
  value_type:
    | 'string'
    | 'string_list'
    | 'number'
    | 'number_list'
    | 'boolean'
    | 'year_range'
    | 'number_range'
    | 'score_range';
  needs_metadata: boolean;
  /** The metadata field it reads, if any. */
  metadata_field: string | null;
  /** Whether an enabled source provides that field: a listed condition may be unanswerable. */
  available: boolean;
  /** The media types it can match on. Radarr reports no `tvdbId`, `seriesType` or seasons. */
  media_types: string[];
  /** The `LibraryFacets` axis its values come from, empty for an open set such as a keyword. */
  suggestions: string;
  /**
   * How its values combine (`any` or `all`), and the kind asking it the other way. Both empty
   * for a single value on the media side, which cannot be "all of".
   */
  quantifier: string;
  counterpart: string;
}

export interface Localization {
  language: string;
  /** `ltr` or `rtl` for `language`, decided by the backend. */
  direction: 'ltr' | 'rtl';
  strings: Record<string, string>;
  /** The placeholders that hold a count, grouped as the language groups digits. */
  counts: string[];
  /** The placeholders that hold a machine format, isolated in a right-to-left sentence. */
  isolated: string[];
  /** The `ui_theme` setting, applied with the strings. */
  theme: string;
}

export interface BackupFile {
  name: string;
  size_bytes: number;
  created_at: string;
  /** Sealed with the backup passphrase: a restore asks for it. */
  encrypted: boolean;
  /** In the copy folder as its last good copy left it. False without one. */
  copied: boolean;
}

/** The copy into `ROUTARR_BACKUP_COPY_DIR`, read from its tasks, never from the folder. */
export interface BackupCopy {
  folder: string;
  /**
   * `unmounted`: the folder lacks the marker its first copy left, a share not mounted.
   * `foreign`: another Routarr database copied into it.
   */
  state: 'never' | 'running' | 'copied' | 'failed' | 'unmounted' | 'foreign';
  /** What the last copy said, in the interface language. */
  detail: string | null;
  copied_at: string | null;
  /** The last good copy found the folder on the disk of the data folder. */
  same_disk: boolean;
}

/** A restore staged for the next start, which applies it whenever it comes. */
export interface PendingRestore {
  name: string;
  staged_at: string;
}

/** The database the last restore replaced, kept until deleted. */
export interface ReplacedDatabase {
  size_bytes: number;
  replaced_at: string;
}

/** What a retention purge removed. */
export interface MaintenanceReport {
  decisions_removed: number;
  logs_removed: number;
  jobs_removed: number;
  metadata_cache_removed: number;
  source_identifiers_removed: number;
  sessions_removed: number;
  security_events_removed: number;
}

/** Which gate the shell must show. Readable without a session, by necessity. */
export interface AuthMode {
  mode: 'none' | 'apikey' | 'forms' | 'external' | 'oidc';
  /** Whether the server holds a key. False in `forms` and `oidc` until one is created. */
  api_key_configured: boolean;
  /** Set through `ROUTARR_API_KEY`, so a key replaced here would last until the next restart. */
  api_key_pinned: boolean;
}

/** Who a request is, which the shell reads once the gate has let the person in. */
export interface Me {
  subject: string;
  /** When the application key the request carries stops being accepted, else null. */
  expires_at: string | null;
  /** A person let in to read alone, refused every other call with 403 `read_only`. */
  read_only: boolean;
}

/**
 * The proof a session gives before it makes or withdraws a key: the password in `forms`, the
 * API key in `apikey`. A header key, or an OpenID Connect sign-in within ten minutes, needs none.
 */
export interface Proof {
  current_password?: string;
  current_key?: string;
}

/** One live session, named by a handle that opens nothing. */
export interface Session {
  handle: string;
  subject: string;
  /** The mode that opened it. */
  source: 'forms' | 'oidc' | 'apikey';
  /** `read` for a person the OpenID Connect read lists let in, who reads alone. */
  reach: 'full' | 'read';
  created_at: string;
  last_used_at: string;
  expires_at: string;
  /** Whether this browser holds it. */
  current: boolean;
}

/** What a password change answers: the new API key, shown once, when every key was revoked. */
export interface PasswordChanged {
  ok: boolean;
  api_key: string | null;
}

/** What a restore answers with: staged, not applied until the next start. */
export interface RestoreResult {
  /** What the archive says of itself. The backup card reads its `includes_master_key`. */
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
  /** How many are kept before the oldest is pruned. */
  retention_count: number;
  /** Null when no copy folder is set. */
  copy: BackupCopy | null;
  pending_restore: PendingRestore | null;
  replaced_database: ReplacedDatabase | null;
}

export interface MetadataProvider {
  id: string;
  display_name: string;
  /** False for a source whose data arrives with the library sync. */
  fetched: boolean;
  needs_key: boolean;
  /** The environment variable its credential comes from, so the interface can name it. */
  key_env: string | null;
  configured: boolean;
  fields: string[];
  /** `movie` and `series`: TheTVDB answers for series alone. */
  media_types: string[];
  /** The site its data comes from, credited and linked. Null for the Arrs. */
  website: string | null;
}

export interface MetadataProviders {
  providers: MetadataProvider[];
  /** Enabled sources, highest priority first. Anything absent is off. */
  order: string[];
}

export interface ConditionCatalog {
  match_modes: MatchMode[];
  conditions: ConditionSpec[];
  /** The scales a `score_range` names. */
  score_scales: ScoreScale[];
}

/** A scale a score is given on: out of 10 or 100, with or without a vote count. */
export interface ScoreScale {
  id: string;
  name: string;
  max: number;
  votes: boolean;
}

/** A score on one scale, and its votes where the scale counts them. */
export interface Rating {
  value: number;
  votes: number | null;
}

export interface ValidationIssue {
  severity: 'error' | 'warning';
  field: string;
  /** The dictionary key the message came from, which names the kind of issue. */
  key: string;
  /** The values the message was written with, by placeholder. */
  params: Record<string, string>;
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

/** Ready-made rules for a usual library split. */
export interface TemplateSummary {
  id: string;
  version: number;
  name: string;
  description: string;
  /** Where its rules send titles, as added or as suggested. Null for `requesters`. */
  category: string | null;
  parameter: 'languages' | 'requesters' | 'quality_profiles' | null;
  /** For `languages`: the languages the household watches in. */
  languages: string[];
  /** For `requesters`: the requesters the library's Seerr tags name. */
  requesters: RequesterTag[];
  /** For `quality_profiles`: the profiles its rule names, or those that read as 4K. */
  quality_profiles: string[];
  /** The sources it needs, none of them enabled. */
  needs: NeededSource[];
  /** The version the rules in place came from, null when it is not added. */
  added_version: number | null;
  rule_ids: string[];
}

export interface NeededSource {
  id: string;
  name: string;
}

export interface RequesterTag {
  tag: string;
  name: string;
  titles: number;
  category: string;
  /** Whether a rule added from the template routes them. */
  added: boolean;
}

export interface TemplateRequest {
  category?: string;
  languages?: string[];
  requesters?: { tag: string; category: string }[];
  quality_profiles?: string[];
  replace?: boolean;
}

export interface PlannedRule {
  rule: RuleDraft;
  issues: ValidationIssue[];
}

export interface TakenBy {
  rule_id: string;
  rule_name: string;
  titles: number;
}

/** The titles a template would decide, by what becomes of them. */
export interface TemplateReach {
  titles: number;
  to_move: number;
  in_place: number;
  unmapped: number;
}

export interface TemplateTitle {
  media_id: string;
  media_title: string;
  media_type: string;
  instance_name: string | null;
  category: string;
  rule_name: string;
  action: DecisionAction;
}

export interface TemplatePreview {
  rules: PlannedRule[];
  /** Categories adding it would create. */
  creates: string[];
  reach: TemplateReach;
  /** The first titles it would decide, those to move first. */
  titles: TemplateTitle[];
  /** The rules in place that would keep titles from it, most first. */
  taken_by: TakenBy[];
}

export interface TemplateAdded {
  rules: Rule[];
  created: string[];
  replaced: number;
}

/** One line of a decision's justification. */
export interface ReasonLine {
  text: string;
  /** True for a condition met or the deciding exception, null for a line stored as a sentence. */
  matched: boolean | null;
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
  /** The category the current root folder leads to, null when it leads to none. */
  current_category: string | null;
  target_root_folder: string | null;
  target_category: string;
  matched_rule_id: string | null;
  matched_rule_name: string | null;
  is_override: boolean;
  reasons: string[];
  /** The same lines, each with whether it states what decided. */
  reason_lines: ReasonLine[];
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
  /** What caused it: `manual`, `schedule` or `webhook`. Null on a row stored without one. */
  actor: string | null;
  /** Who asked. Null for the scheduler, a mode that names nobody, or a row stored without one. */
  subject: string | null;
  /** Whether a revert may undo this move: only the latest standing move of a title. */
  revertible: boolean;
}

export interface SimulationResult extends SimulationSummary {
  /** One entry per destination this plan feeds, worst first. */
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
  /** Moves of files the Arr was still making: requested, settled by the next sync. */
  moving: number;
  /** Selected proposals a newer simulation replaced with another move. */
  superseded: number;
  stopped: StopReason | null;
  errors: { decision_id: string; media_title: string; message: string }[];
}

/** Which titles an apply or a revert of many covers, as the decisions list filters them. */
export interface DecisionFilter {
  instance_id?: string;
  media_type?: string;
  category?: string;
  current_root_folder?: string;
  rule_id?: string;
  search?: string;
}

/** Outcome of a run in slices: a whole simulation applied, or what a filter matches reverted. */
export interface BatchApplyReport {
  candidates: number;
  applied: number;
  failed: number;
  skipped: number;
  batches_run: number;
  batches_planned: number;
  moving: number;
  superseded: number;
  /** A slice was refused whole, or the run was stopped, and the rest abandoned. */
  stopped_early: boolean;
  stopped: StopReason | null;
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
  tvdb_id: number | null;
  imdb_id: string | null;
  current_root_folder: string | null;
  monitored: boolean;
  has_files: boolean;
  status: string | null;
  last_synced_at: string | null;
  computed_category: string | null;
  override_category: string | null;
  has_metadata: boolean;
}

/** Every enabled source collapsed in priority order, not one provider's answer. */
export interface MediaMetadata {
  genres: string[];
  keywords: string[];
  original_language: string | null;
  origin_countries: string[];
  certification: string | null;
  /** The system `certification` belongs to: an ISO 3166-1 country code, or `MAL`. */
  certification_scale: string | null;
  /** In the words Radarr and Sonarr use: `released`, `continuing`, `ended`... */
  status: string | null;
  overview: string | null;
  /** Each scale's score, by scale id. */
  ratings: Record<string, Rating>;
  /** Field name -> the source that supplied it, `ratings.<scale>` for a score. */
  field_sources: Record<string, string>;
  /** Sources that supplied at least one field, highest priority first. */
  sources: string[];
}

export interface ConditionOutcome {
  kind: string;
  matched: boolean;
  /** The dictionary key and values `expected` was written from. */
  key: string;
  params: Record<string, string>;
  expected: string;
  observed: string;
  /** Which metadata source the observed value came from, when it reads one. */
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

/**
 * The stored media row an explanation carries, not a library list item. Its list fields are
 * JSON arrays in a string, as stored.
 */
export interface ExplainedMedia {
  id: string;
  instance_id: string;
  arr_id: number;
  media_type: MediaType;
  title: string;
  sort_title: string | null;
  year: number | null;
  tmdb_id: number | null;
  tvdb_id: number | null;
  imdb_id: string | null;
  current_path: string | null;
  current_root_folder: string | null;
  monitored: boolean;
  has_files: boolean;
  status: string | null;
  added_at: string | null;
  series_type: string | null;
  size_on_disk: number | null;
  season_count: number | null;
  tags: string | null;
  genres: string | null;
  original_language: string | null;
  certification: string | null;
  quality_profile: string | null;
  /** Minutes: a film's, a series' episode's. */
  runtime: number | null;
  studio: string | null;
  network: string | null;
  collection: string | null;
  collection_tmdb_id: number | null;
  /** The scores Radarr reports, by scale, as a JSON object in a string. */
  ratings: string | null;
  last_synced_at: string | null;
  /** The Arr's tag labels: `tags` holds the same as a JSON string. */
  tag_list: string[];
  /** The genres the Arr reports: `genres` holds the same as a JSON string. */
  genre_list: string[];
}

export interface Explanation {
  media: ExplainedMedia;
  metadata: MediaMetadata | null;
  override_category: string | null;
  target_category: string;
  /** What the rules alone decide, the exception left out: what a rule test pins. */
  rules_category: string;
  target_root_folder: string | null;
  /** The category the current root folder leads to, which keeping the title pins. */
  current_category: string | null;
  action: DecisionAction;
  confidence: number;
  winning_rule: string | null;
  rule_traces: RuleTrace[];
  /** False when no run reads the title's instance, which is switched off. */
  instance_enabled: boolean;
  /** How each source that finds a work by search found this title: `mapping`, `shared` or `search`. */
  identified_by: Record<string, string>;
}

/** What keeping a title where it is takes. */
export interface KeepRequest {
  reason?: string;
}

export interface OverrideEntry {
  id: string;
  media_id: string;
  target_category: string;
  reason: string | null;
  created_at: string;
  /** Who set it: an application's name, or the person a sign-in mode names. */
  subject: string | null;
  media_title: string;
  media_type: MediaType;
  instance_name: string;
}

export interface Job {
  id: string;
  kind: string;
  status: 'running' | 'success' | 'failed' | 'cancelled';
  trigger: string;
  /** Who asked: an application's name, or the person a sign-in mode names. */
  subject: string | null;
  instance_id: string | null;
  detail: string | null;
  progress_current: number;
  progress_total: number;
  error_message: string | null;
  /** The code the call would have answered, for a task that failed on an error. */
  error: string | null;
  started_at: string;
  finished_at: string | null;
  /** The report a finished task answered, when a call started it. */
  result: unknown;
}

/** The envelope every refusal and failure of the API answers. */
export interface ErrorBody {
  error: string;
  message: string;
  /** Why a `conflict` refuses: a running task, or a state somebody has to change. */
  reason?: string;
  /** The guardrail that asks, for `confirmation_required`. */
  confirm?: string;
  /** Beside `confirm`: whether this caller may send the name back. */
  answerable?: boolean;
  /** Beside `confirm`: the other guardrails the question states. */
  includes?: string[];
}

/** Whether the notification webhook is signed, and since when. */
export interface WebhookSigningStatus {
  signed: boolean;
  since: string | null;
  /** False when a secret is set that this installation cannot open. */
  readable: boolean;
}

/** A scope beyond read, which every application key holds. */
export type ApplicationScope = 'operate' | 'write' | 'configure';

/** The names a guardrail asks under, as the executor sends them back. */
export type Guardrail = 'capacity' | 'threshold' | 'batch' | 'unreachable';

export interface Application {
  id: string;
  name: string;
  scopes: ApplicationScope[];
  may_confirm: Guardrail[];
  may_move_files: boolean;
  created_at: string;
  created_by: string | null;
  last_used_at: string | null;
  /** Null for a key that never expires. */
  expires_at: string | null;
  /** Past its expiry, every call refused until it is revoked. */
  expired: boolean;
}

export interface NewApplication {
  name: string;
  scopes: ApplicationScope[];
  may_confirm: Guardrail[];
  may_move_files: boolean;
  /** Days the key is accepted for, null for ever. */
  expires_in_days: number | null;
}

/** A key just made. The token is in this answer and never again. */
export interface MintedApplication extends Application {
  token: string;
}

/** What a security event is about, as `services::audit::Kind` names it. */
export type SecurityEventKind =
  | 'sign_in'
  | 'sign_out'
  | 'api_key'
  | 'application_key'
  | 'scope'
  | 'origin'
  | 'password'
  | 'proof'
  | 'backup'
  | 'restore'
  | 'configuration'
  | 'settings'
  | 'signing_secret'
  | 'security_log';

/** One event of the security log, which only the owner reads. */
export interface SecurityEvent {
  id: number;
  at: string;
  kind: SecurityEventKind;
  outcome: 'allowed' | 'refused';
  subject: string | null;
  client: string | null;
  /** A dictionary key, read with `params`. */
  message: string;
  params: Record<string, string>;
  /** How many more like it the same address sent in the minute after. */
  repeated: number;
}

export interface LogEntry {
  id: string;
  decision_id: string | null;
  action: LogAction;
  details: string | null;
  success: boolean;
  error_message: string | null;
  instance_id: string | null;
  media_id: string | null;
  media_title: string | null;
  /** What set the write off: `manual`, `schedule` or `webhook`. */
  actor: string | null;
  /** Who asked, when the mode vouched for a name. */
  subject: string | null;
  /** The task the write ran in, as a move notification names it. */
  task_id: string | null;
  executed_at: string;
}

/** One warning, in the reader's language. */
export interface Warning {
  /** What the warning is about, stable across releases and languages. */
  code: string;
  message: string;
  /** `error` for a fault, `warning` for a configuration to look at, `info` for a fact. */
  severity: 'error' | 'warning' | 'info';
  /** The getting-started step this warning restates, whose banner says it too. */
  guide_step: OnboardingStep['id'] | null;
}

export interface Status {
  version: string;
  dry_run: boolean;
  running_jobs: number;
  pending_decisions: number;
  failed_decisions: number;
  warnings: Warning[];
}

export interface Health {
  /** `ok`, `degraded` for a fault among the warnings, `failing` for a database down. */
  status: string;
  version: string;
  database: string;
  warnings: Warning[];
  instances: {
    id: string;
    name: string;
    instance_type: string;
    status: string;
    /** Why the probe failed, what to change, in the interface language. */
    detail: string | null;
    version: string | null;
    last_sync: string | null;
    last_sync_status: string | null;
    media_count: number;
    mapped_root_folders: number;
    last_webhook_at: string | null;
    last_webhook_event: string | null;
  }[];
  metadata: {
    providers: {
      id: string;
      display_name: string;
      needs_key: boolean;
      configured: boolean;
      connected: boolean | null;
      /** Why a probed source did not answer: `key_refused`, `unreachable`... */
      reason: string | null;
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

/** Values typed in the instance form, tried before anything is saved. */
export interface InstanceProbe {
  instance_type: 'radarr' | 'sonarr';
  base_url: string;
  api_key: string;
  /** The instance being edited, whose stored key a blank `api_key` stands for. */
  id: string | undefined;
}

/** What a connection probe reports. `app_name` is null for an Arr that does not say. */
export interface TestConnectionResponse {
  success: boolean;
  version: string;
  app_name: string | null;
  root_folders: number;
  inaccessible_root_folders: number;
  /** What this release of the Arr lacks, below the oldest Routarr supports. */
  warning: string | null;
  /** The webhook connection of the saved instance tried. Null for values not saved yet. */
  webhook: WebhookCheck | null;
}

/** What a pass reading a source's answers again did. */
export interface EnrichmentReport {
  /** Titles a source that searches for its own identifiers looked for. */
  searched: number;
  /** Answers the pass set out to read. */
  considered: number;
  enriched: number;
  failed: number;
  /** Never asked: the source was taken for down during the pass. */
  skipped: number;
  /** Left to a later day: the source's daily quota was spent. */
  deferred: number;
}

/** What asking the sources again about one title found. */
export interface TitleRefresh {
  /** The sources that gave an answer. */
  answered: number;
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

/** `GET /settings`: every value as text, a sealed one replaced by its `<key>_configured` flag. */
export type StoredSettings = Record<string, string | boolean>;

/** Whether the getting-started guide is still wanted. */
export type OnboardingState = 'pending' | 'dismissed' | 'done';

/** One step of the getting-started guide, read from the installation's data. */
export interface OnboardingStep {
  id: 'instance' | 'categories' | 'metadata' | 'rule' | 'simulation' | 'live';
  done: boolean;
  /** An optional step never holds the guide back from completing. */
  optional: boolean;
}

/** Where a new installation stands, and whether the guide still shows. */
export interface OnboardingStatus {
  state: OnboardingState;
  /** Every step that is not optional is done. */
  complete: boolean;
  steps: OnboardingStep[];
}

/** What a configuration restore managed, and what it could not. */
export interface ConfigImportReport {
  settings: number;
  categories: number;
  instances: number;
  root_folders: number;
  overrides: number;
  rules: number;
  /** Everything not restored, and why. */
  skipped: string[];
  /** The instances restored disabled, by name, each waiting for its API key. */
  needs_key: string[];
  /** Each rule restored otherwise than the bundle has it, with how. */
  adjusted: string[];
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
  /** What the last run found. Null until one reads the case. */
  last_passed: boolean | null;
  last_checked_at: string | null;
  subject: string | null;
}

export interface RuleTestResult {
  id: string;
  name: string;
  expected_category: string;
  /** Where the engine puts it on this run. Null when the stored case cannot be read. */
  actual_category: string | null;
  passed: boolean;
  matched_rule: string | null;
  /** Why the stored case cannot be replayed, in the interface language. */
  error: string | null;
  /** The same, as a key a program reads. */
  error_key: string | null;
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
  /** Bytes crossing from another filesystem, the only traffic that consumes space. */
  incoming_bytes: number;
  /** Bytes moving within one filesystem, where a move is a rename and costs nothing. */
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
  /** Set only when the rule never wins: the rule taking most of what it matched. */
  shadowed_by: string | null;
  matched_nothing: boolean;
  /** Another rule with the same name *and* priority: the tie falls to the id. */
  ambiguous_with: string | null;
  /** Another rule with identical conditions and target. */
  duplicate_of: string | null;
}

export interface RuleHealthReport {
  total_media: number;
  rules: RuleHealth[];
}

/** One value present in the library, and how many items carry it. */
export interface Facet {
  /** What a rule stores and the engine compares: a language matches on `ja`, however shown. */
  value: string;
  /** What to show instead of `value`, where they differ. Absent for a genre, its own label. */
  label?: string;
  count: number;
  /** What the value means, where several mean one thing, as countries' certifications do. */
  group?: string;
}

/**
 * Values a condition may hold that the library does not define. A language comes from a fixed
 * table, and offering only those already synced would hide the rest.
 */
export interface Vocabularies {
  original_languages: Facet[];
  origin_countries: Facet[];
}

/** What the library actually holds, per axis a condition can read. */
export interface LibraryFacets {
  total_media: number;
  vocabularies: Vocabularies;
  /** Titles no source describes in any field a metadata condition reads. */
  without_metadata: number;
  genres: Facet[];
  original_languages: Facet[];
  origin_countries: Facet[];
  certifications: Facet[];
  tags: Facet[];
  series_types: Facet[];
  /** Every profile the Arrs define, an unused one counted 0. */
  quality_profiles: Facet[];
  studios: Facet[];
  networks: Facet[];
  collections: Facet[];
  root_folders: Facet[];
  /** The statuses the Arrs give the titles, as they write them. */
  statuses: Facet[];
}

/**
 * The axes of `LibraryFacets` that carry counted values, derived so a new axis joins them.
 * Widened to `Record<string, Facet[]>`, a renamed axis would draw an empty picker silently.
 */
export type FacetAxis = {
  [K in keyof LibraryFacets]: LibraryFacets[K] extends Facet[] ? K : never;
}[keyof LibraryFacets];

/** A rule template a folder suggestion offers, by id and by its name in the interface language. */
export interface TemplateOffer {
  id: string;
  name: string;
}

/** A category suggested for a folder no category leads to, read from its name. */
export interface FolderSuggestion {
  root_folder_id: string;
  instance_id: string;
  instance_name: string;
  path: string;
  /** Null when the name says several categories and the person picks. */
  category: string | null;
  choices: string[];
  /** Why, in the interface language. */
  reason: string;
  /** Offered while the library holds none of the template's rules. */
  template: TemplateOffer | null;
  dismissed: boolean;
}

export interface Dismissal {
  dismissed: boolean;
}

/** The anime identifier table, as the Metadata sources screen shows it. */
export interface AnimeIdsState {
  /** Null before a first load. */
  loaded_at: string | null;
  entries: number | null;
  /** The address it was downloaded from, or `file`. */
  origin: string | null;
  /** Where a download asks. */
  address: string;
}

/** What a load of the anime identifier table did. */
export interface AnimeIdsLoaded {
  entries: number;
  /** Titles whose identifiers the table gave or changed. */
  titles: number;
}
