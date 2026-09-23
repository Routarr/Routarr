// REST client for the Routarr backend.

import { basePath } from './basePath';

import type {
  AuthMode,
  ApplyReport,
  BatchApplyReport,
  ConfigImportReport,
  Category,
  ConditionCatalog,
  Decision,
  Explanation,
  Health,
  Instance,
  Job,
  LogEntry,
  MaintenanceReport,
  MappingConflict,
  BackupFile,
  BackupList,
  Localization,
  MediaListItem,
  MetadataProviders,
  OverrideEntry,
  Paginated,
  RootFolder,
  Rule,
  RuleDraft,
  RestoreResult,
  RulePreview,
  Settings,
  SimulationResult,
  Status,
  SyncReport,
  TestConnectionResponse,
  ValidationIssue,
  LibraryFacets,
  RuleHealthReport,
  RuleTest,
  RuleTestRun,
} from './types';

// Resolved once: the mount point cannot change while the page is loaded, and
// recomputing it per request would parse a URL on every call.
const API_BASE = `${basePath()}/api/v1`;
const API_KEY_STORAGE = 'routarr.apiKey';

/** Thrown for any non-2xx response, carrying the backend's message. */
export class ApiError extends Error {
  // Declared and assigned rather than written as constructor parameter
  // properties: those are the one piece of TypeScript that cannot be erased,
  // since they compile to assignments. Vite strips types per file and emits
  // nothing else, so `erasableSyntaxOnly` holds the source to what that
  // pipeline can actually honour.
  readonly status: number;
  readonly kind: string;
  /// The `X-Request-Id` the server answered with, so a failure on screen can be
  /// matched to the line it left in the log. Null for a request that never
  /// reached it.
  readonly requestId: string | null;
  /**
   * Which guardrail is asking, for a refusal that can be answered.
   *
   * Three of them ask, and the caller sends this name back to say what it
   * looked at. A bare "yes" answered all three at once, so confirming a
   * capacity shortfall lifted the batch threshold the operator was never
   * shown.
   */
  readonly confirm: string | null;

  constructor(
    message: string,
    status: number,
    kind: string,
    requestId: string | null = null,
    confirm: string | null = null,
  ) {
    super(message);
    this.status = status;
    this.kind = kind;
    this.requestId = requestId;
    this.confirm = confirm;
    this.name = 'ApiError';
  }

  /** True when the backend is asking for an explicit confirmation. */
  get needsConfirmation(): boolean {
    // Keyed on the stable code, not on the message text: matching prose would
    // break on any rewording.
    return this.kind === 'confirmation_required';
  }
}

export function getApiKey(): string {
  return localStorage.getItem(API_KEY_STORAGE) ?? '';
}

export function setApiKey(key: string): void {
  if (key.trim()) localStorage.setItem(API_KEY_STORAGE, key.trim());
  else localStorage.removeItem(API_KEY_STORAGE);
}

/** Long enough for a simulation over a large library, short enough to be a signal. */
const REQUEST_TIMEOUT_MS = 30_000;

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const key = getApiKey();
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(options.headers as Record<string, string> | undefined),
  };
  // Only sent when the user configured one; an open instance ignores it.
  if (key) headers['X-Api-Key'] = key;

  // A server that never answers must not leave a spinner running forever. The
  // Arr calls the backend makes are bounded on its side; this bounds the one
  // the browser makes to the backend. The kind is what `describeError` reads,
  // so the wording stays with the other translated messages.
  //
  // Composed with the caller's signal rather than replaced by it: written as a
  // fallback, passing a signal to cancel a superseded load silently removed the
  // timeout from that request, which is the one request most likely to be
  // waiting on a host that never answers. `any` propagates the reason of
  // whichever fired, so a timeout still arrives as `TimeoutError` and a
  // cancellation as `AbortError`.
  //
  // The `instanceof` is not belt and braces over a typed parameter: the
  // endpoint sweep in `client.test.ts` calls every method on this object with
  // positional placeholders, so a signal parameter receives a string there.
  // `AbortSignal.any` throws on one, and the method would then answer without
  // issuing the request the sweep exists to inspect — a whole class of URL
  // faults going unchecked to spare one line here.
  const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS);
  const caller = options.signal instanceof AbortSignal ? options.signal : null;
  const signal = caller ? AbortSignal.any([caller, timeout]) : timeout;
  let res: Response;
  try {
    res = await fetch(`${API_BASE}${path}`, { ...options, headers, signal });
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === 'TimeoutError') {
      throw new ApiError('', 0, 'timeout');
    }
    throw cause;
  }

  if (!res.ok) {
    const body = await res.text();
    let message = body || res.statusText;
    let kind = 'http_error';
    let confirm: string | null = null;
    try {
      const json = JSON.parse(body);
      message = json.message ?? json.error ?? message;
      kind = json.error ?? kind;
      confirm = typeof json.confirm === 'string' ? json.confirm : null;
    } catch {
      // Not a JSON error envelope (proxy error page, empty body): keep the text.
    }
    throw new ApiError(
      message,
      res.status,
      kind,
      res.headers?.get?.('x-request-id') ?? null,
      confirm,
    );
  }

  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

const body = (data: unknown) => JSON.stringify(data ?? {});
const query = (params?: Record<string, string | number | boolean | undefined>) => {
  if (!params) return '';
  const entries = Object.entries(params).filter(([, v]) => v !== undefined && v !== '');
  if (entries.length === 0) return '';
  return '?' + new URLSearchParams(entries.map(([k, v]) => [k, String(v)])).toString();
};

export type QueryParams = Record<string, string | number | boolean | undefined>;

export const api = {
  // ---------------------------------------------------------- health & jobs
  getLocalization: () => request<Localization>('/localization'),
  getLanguages: (signal?: AbortSignal) =>
    request<{
      default: string;
      /** `completion` is the share of English keys translated, 0–100. */
      languages: { code: string; name: string; completion: number; direction: 'ltr' | 'rtl' }[];
    }>('/localization/languages', { signal }),
  getStatus: (signal?: AbortSignal) => request<Status>('/status', { signal }),
  /**
   * `probe: false` answers from the database alone. The probe is what costs
   * five seconds when an Arr is unreachable, and every instance then reports
   * `status: 'unchecked'` rather than a guess.
   */
  getHealth: (options: { probe?: boolean } = {}, signal?: AbortSignal) =>
    request<Health>(`/health${options.probe === false ? '?probe=false' : ''}`, { signal }),
  getJobs: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<Job>>(`/jobs${query(params)}`, { signal }),
  purge: () => request<MaintenanceReport>('/maintenance/purge', { method: 'POST' }),

  // ---------------------------------------------------------- instances
  getInstances: (signal?: AbortSignal) => request<Instance[]>('/instances', { signal }),
  createInstance: (data: unknown) =>
    request<Instance>('/instances', { method: 'POST', body: body(data) }),
  updateInstance: (id: string, data: unknown) =>
    request<Instance>(`/instances/${id}`, { method: 'PUT', body: body(data) }),
  deleteInstance: (id: string) => request<unknown>(`/instances/${id}`, { method: 'DELETE' }),
  testInstance: (id: string) =>
    request<TestConnectionResponse>(`/instances/${id}/test`, { method: 'POST' }),
  syncInstance: (id: string) => request<SyncReport>(`/instances/${id}/sync`, { method: 'POST' }),
  syncAll: () => request<SyncReport[]>('/instances/sync', { method: 'POST' }),
  rotateWebhookToken: (id: string) =>
    request<Instance>(`/instances/${id}/webhook-token`, { method: 'POST' }),

  // ---------------------------------------------------------- root folders
  getRootFolders: (signal?: AbortSignal) => request<RootFolder[]>('/root-folders', { signal }),
  /** Declare a destination the instance does not report as a root folder. */
  declareRootFolder: (instance_id: string, path: string) =>
    request<{ id: string; path: string; verified: boolean }>('/root-folders', {
      method: 'POST',
      body: body({ instance_id, path }),
    }),
  deleteRootFolder: (id: string) =>
    request<unknown>(`/root-folders/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  getMappingConflicts: (signal?: AbortSignal) =>
    request<MappingConflict[]>('/root-folders/conflicts', { signal }),
  updateRootFolderCategory: (id: string, category: string | null) =>
    request<unknown>(`/root-folders/${id}/category`, {
      method: 'PUT',
      body: body({ category }),
    }),

  // ---------------------------------------------------------- categories
  getCategories: (signal?: AbortSignal) => request<Category[]>('/categories', { signal }),
  createCategory: (data: unknown) =>
    request<Category>('/categories', { method: 'POST', body: body(data) }),
  renameCategory: (id: string, name: string) =>
    request<Category>(`/categories/${id}`, { method: 'PUT', body: body({ name }) }),
  deleteCategory: (id: string) => request<unknown>(`/categories/${id}`, { method: 'DELETE' }),

  // ---------------------------------------------------------- rules
  getRules: (signal?: AbortSignal) => request<Rule[]>('/rules', { signal }),

  getRuleHealth: (signal?: AbortSignal) => request<RuleHealthReport>('/rules/health', { signal }),
  getLibraryFacets: (signal?: AbortSignal) => request<LibraryFacets>('/media/facets', { signal }),

  getRuleTests: (signal?: AbortSignal) => request<RuleTest[]>('/rule-tests', { signal }),
  runRuleTests: () => request<RuleTestRun>('/rule-tests/run', { method: 'POST' }),
  pinRuleTest: (name: string, mediaId: string, expectedCategory?: string) =>
    request<RuleTest>('/rule-tests', {
      method: 'POST',
      body: JSON.stringify({
        name,
        media_id: mediaId,
        expected_category: expectedCategory,
      }),
    }),
  deleteRuleTest: (id: string) =>
    request<{ deleted: string }>(`/rule-tests/${id}`, { method: 'DELETE' }),
  getConditionCatalog: (signal?: AbortSignal) =>
    request<ConditionCatalog>('/rules/conditions', { signal }),
  createRule: (data: RuleDraft) => request<Rule>('/rules', { method: 'POST', body: body(data) }),
  updateRule: (id: string, data: RuleDraft) =>
    request<Rule>(`/rules/${id}`, { method: 'PUT', body: body(data) }),
  deleteRule: (id: string) => request<unknown>(`/rules/${id}`, { method: 'DELETE' }),
  duplicateRule: (id: string) => request<Rule>(`/rules/${id}/duplicate`, { method: 'POST' }),
  reorderRules: (rule_ids: string[]) =>
    request<unknown>('/rules/reorder', { method: 'POST', body: body({ rule_ids }) }),
  authMode: (signal?: AbortSignal) => request<AuthMode>('/auth/mode', { signal }),
  login: (username: string, password: string) =>
    request<{ username: string }>('/auth/login', {
      method: 'POST',
      body: body({ username, password }),
    }),
  logout: () => request<{ ok: boolean }>('/auth/logout', { method: 'POST' }),
  /// The new key comes back exactly once — there is no route that reads it
  /// again, so a caller that drops it has to mint another.
  rotateApiKey: () => request<{ api_key: string }>('/auth/api-key', { method: 'POST' }),
  deleteApiKey: () => request<unknown>('/auth/api-key', { method: 'DELETE' }),

  validateRule: (data: RuleDraft) =>
    request<{ valid: boolean; issues: ValidationIssue[] }>('/rules/validate', {
      method: 'POST',
      body: body(data),
    }),
  previewRule: (rule: RuleDraft, ruleId?: string) =>
    request<RulePreview>('/rules/preview', {
      method: 'POST',
      body: body({ rule, rule_id: ruleId ?? null }),
    }),
  exportRules: () => request<RuleBundle>('/rules/export'),

  // ---------------------------------------------------------- configuration
  /** Everything that cannot be regenerated from the Arrs. Never carries a key. */
  exportConfig: () => request<unknown>('/config/export'),
  importConfig: (bundle: unknown) =>
    request<ConfigImportReport>('/config/import', { method: 'POST', body: body({ bundle }) }),

  importRules: (bundle: RuleBundle, replace: boolean) =>
    request<{ imported: number; skipped: string[] }>('/rules/import', {
      method: 'POST',
      body: body({ bundle, replace }),
    }),

  // ---------------------------------------------------------- media
  getMedia: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<MediaListItem>>(`/media${query(params)}`, { signal }),
  explainMedia: (id: string) => request<Explanation>(`/media/${id}/explain`),

  // ---------------------------------------------------------- decisions
  runSimulation: (data?: unknown) =>
    request<SimulationResult>('/simulate', { method: 'POST', body: body(data) }),
  getDecisions: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<Decision>>(`/decisions${query(params)}`, { signal }),
  /** `confirm` names the guardrails already answered, not a blanket yes. */
  applyDecisions: (decision_ids: string[], move_files = false, confirm: string[] = []) =>
    request<ApplyReport>('/decisions/apply', {
      method: 'POST',
      body: body({ decision_ids, move_files, confirm }),
    }),
  /** Apply everything one simulation proposed, in slices of `batch_limit`. */
  applyAllDecisions: (simulation_id: string, move_files = false, confirm: string[] = []) =>
    request<BatchApplyReport>('/decisions/apply-all', {
      method: 'POST',
      body: body({ simulation_id, move_files, confirm }),
    }),

  revertDecisions: (decision_ids: string[], move_files = false) =>
    request<ApplyReport>('/decisions/revert', {
      method: 'POST',
      body: body({ decision_ids, move_files }),
    }),

  // ---------------------------------------------------------- overrides
  getOverrides: (signal?: AbortSignal) => request<OverrideEntry[]>('/overrides', { signal }),
  createOverride: (data: unknown) =>
    request<OverrideEntry>('/overrides', { method: 'POST', body: body(data) }),
  deleteOverride: (id: string) => request<unknown>(`/overrides/${id}`, { method: 'DELETE' }),

  // ---------------------------------------------------------- logs
  getLogs: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<LogEntry>>(`/logs${query(params)}`, { signal }),
  /// A link the browser follows itself: the sign-in has to leave this origin,
  /// so it cannot be a fetch.
  oidcStartUrl: () => `${API_BASE}/auth/oidc/start`,
  logsExportUrl: (params?: QueryParams) => `${API_BASE}/logs/export${query(params)}`,

  // ---------------------------------------------------------- settings
  getSettings: (signal?: AbortSignal) => request<Settings>('/settings', { signal }),
  getMetadataProviders: (signal?: AbortSignal) =>
    request<MetadataProviders>('/metadata/providers', { signal }),
  listBackups: (signal?: AbortSignal) => request<BackupList>('/backups', { signal }),
  createBackup: () => request<BackupFile>('/backups', { method: 'POST', body: body({}) }),
  deleteBackup: (name: string) =>
    request<unknown>(`/backups/${encodeURIComponent(name)}`, { method: 'DELETE' }),
  restoreBackup: (name: string) =>
    request<RestoreResult>(`/backups/${encodeURIComponent(name)}/restore`, {
      method: 'POST',
      body: body({}),
    }),
  /// The archive carries the master key, so it is fetched with the API key like
  /// everything else rather than linked to directly.
  downloadBackup: async (name: string): Promise<Blob> => {
    const key = getApiKey();
    const response = await fetch(`${API_BASE}/backups/${encodeURIComponent(name)}`, {
      headers: key ? { 'X-Api-Key': key } : {},
    });
    if (!response.ok) {
      throw new ApiError(await response.text(), response.status, 'download_failed');
    }
    return response.blob();
  },
  updateSettings: (settings: Settings) =>
    request<unknown>('/settings', { method: 'PUT', body: body({ settings }) }),
};

export interface RuleBundle {
  version: number;
  exported_at?: string | null;
  rules: RuleDraft[];
  categories: string[];
}
