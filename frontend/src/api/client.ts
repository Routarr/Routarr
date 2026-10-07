// REST client for the Routarr backend. Every request goes through `api` below.
//
// Nothing in `frontend/src/api/` imports Svelte or `frontend/src/lib/`. `lib/`
// builds on these modules, and they stay plain TypeScript that a test runs
// without rendering anything.

import { basePath } from './basePath';
import type { OpenApiDocument } from './openapi';

import type {
  Application,
  AuthMode,
  ApplyReport,
  BatchApplyReport,
  ConfigImportReport,
  Category,
  ConditionCatalog,
  Decision,
  ErrorBody,
  Explanation,
  Health,
  Instance,
  InstanceProbe,
  Job,
  LogEntry,
  MaintenanceReport,
  MappingConflict,
  BackupFile,
  BackupList,
  Localization,
  MediaListItem,
  MetadataProviders,
  MintedApplication,
  NewApplication,
  OnboardingState,
  OnboardingStatus,
  OverrideEntry,
  Paginated,
  RootFolder,
  PasswordChanged,
  Proof,
  Rule,
  RuleDraft,
  SecurityEvent,
  Session,
  RestoreResult,
  RulePreview,
  Settings,
  StoredSettings,
  SimulationResult,
  Status,
  WebhookSigningStatus,
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

/**
 * Where this browser's storage holds a key, which any script on the page can
 * read. Found there, it is exchanged once for a session cookie, which no
 * script can, and removed.
 */
const STORED_KEY = 'routarr.apiKey';

/** Thrown for any non-2xx response, carrying the backend's message. */
export class ApiError extends Error {
  // Declared and assigned rather than written as constructor parameter
  // properties: those cannot be erased, since they compile to assignments.
  // Vite strips types per file and emits nothing else, so `erasableSyntaxOnly`
  // holds the source to what that pipeline can actually honour.
  readonly status: number;
  readonly kind: string;
  /**
   * The `X-Request-Id` the server answered with, so a failure on screen can be
   * matched to the line it left in the log. Null for a request that never
   * reached it.
   */
  readonly requestId: string | null;
  /**
   * Which guardrail is asking, for a refusal that can be answered.
   *
   * Three of them ask, and the caller sends this name back to say what it
   * looked at. A bare "yes" would answer all three at once, and confirming a
   * capacity shortfall would lift the batch threshold the operator was never
   * shown.
   */
  readonly confirm: string | null;
  /**
   * The other guardrails the question states, as the batch question states a
   * sleeping or full destination. Answering the question sends them back with
   * `confirm`, or the server asks again.
   */
  readonly includes: string[];

  constructor(
    message: string,
    status: number,
    kind: string,
    requestId: string | null = null,
    confirm: string | null = null,
    includes: string[] = [],
  ) {
    super(message);
    this.status = status;
    this.kind = kind;
    this.requestId = requestId;
    this.confirm = confirm;
    this.includes = includes;
    this.name = 'ApiError';
  }

  /** True when the backend is asking for an explicit confirmation. */
  get needsConfirmation(): boolean {
    // Keyed on the stable code, not on the message text: matching prose would
    // break on any rewording.
    return this.kind === 'confirmation_required';
  }
}

let storedKeyRead = false;
let exchanging: Promise<void> | null = null;

/**
 * The exchange of a key found in storage, started by the first request and
 * awaited by every request made while it runs. `null` once there is nothing to
 * wait for, so a request with nothing stored leaves at once.
 */
function storedKeyExchange(): Promise<void> | null {
  if (!storedKeyRead) {
    storedKeyRead = true;
    let stored: string | null = null;
    try {
      stored = localStorage.getItem(STORED_KEY);
      localStorage.removeItem(STORED_KEY);
    } catch {
      // Storage refused, as in a private window: there is nothing to remove.
    }
    if (stored) {
      const key = stored;
      exchanging = exchange<unknown>(
        '/auth/key-session',
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ key }),
          signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
        },
        readJson,
      )
        // A key that opens nothing leaves the gate to ask for one.
        .then(
          () => undefined,
          () => undefined,
        )
        .finally(() => {
          exchanging = null;
        });
    }
  }
  return exchanging;
}

/**
 * `AbortSignal.any` where the browser has it, and the same composition by hand
 * where it does not. It is Safari 17.4, Chrome 116 and Firefox 124, and every
 * load passes through it: called bare, an older browser, an iPad held on
 * iPadOS 16 or Firefox ESR 115, throws a `TypeError` on every screen. Whichever
 * signal fires first lends its reason, so a timeout still reads as a timeout.
 */
function anySignal(signals: AbortSignal[]): AbortSignal {
  if (typeof AbortSignal.any === 'function') return AbortSignal.any(signals);
  const composed = new AbortController();
  for (const signal of signals) {
    if (signal.aborted) {
      composed.abort(signal.reason);
      break;
    }
    signal.addEventListener('abort', () => composed.abort(signal.reason), { once: true });
  }
  return composed.signal;
}

/**
 * Long enough for any call answered at once, short enough to be a signal. Work
 * that may run longer, a simulation or an apply over a whole library, is
 * followed through its job (`followed`) and never meets it.
 */
const REQUEST_TIMEOUT_MS = 30_000;

const readJson = (response: Response) => response.json() as Promise<never>;
/** A file the API serves, whatever its type: the one read held to no JSON. */
const readBlob = (response: Response) => response.blob() as Promise<never>;

async function request<T>(
  path: string,
  options: RequestInit = {},
  read: (response: Response) => Promise<T> = readJson,
): Promise<T> {
  const pending = storedKeyExchange();
  if (pending) await pending;
  // `Accept` asks a proxy in front for a 401 rather than a redirect to its
  // sign-in page, which several of them only answer to a browser's navigation.
  const headers: Record<string, string> = {
    Accept: 'application/json',
    'Content-Type': 'application/json',
    ...(options.headers as Record<string, string> | undefined),
  };

  // A server that never answers must not leave a spinner running forever. The
  // Arr calls the backend makes are bounded on its side. This bounds the one
  // the browser makes to the backend. The kind is what `describeError` reads,
  // so the wording stays with the other translated messages.
  //
  // Composed with the caller's signal, never replaced by it: replaced, the
  // request most likely to be waiting on a host that never answers would lose
  // its bound the moment it became cancellable. Whichever signal fires first
  // lends its reason, so a timeout still arrives as `TimeoutError` and a
  // cancellation as `AbortError`.
  //
  // The `instanceof` is not belt and braces over a typed parameter: the
  // endpoint sweep in `client.test.ts` calls every method on this object with
  // positional placeholders, so a signal parameter receives a string there.
  // `AbortSignal.any` throws on one, and the method would then answer without
  // issuing the request the sweep exists to inspect, leaving a whole class of
  // URL faults unchecked to spare one line here.
  const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS);
  const caller = options.signal instanceof AbortSignal ? options.signal : null;
  const signal = caller ? anySignal([caller, timeout]) : timeout;
  try {
    return await exchange<T>(path, { ...options, headers, signal }, read);
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === 'TimeoutError') {
      throw new ApiError('', 0, 'timeout');
    }
    // A failed connection, from `fetch` or a body read. Its message is the
    // browser's, in the browser's language whatever the interface speaks.
    if (cause instanceof TypeError) throw new ApiError('', 0, 'unreachable');
    throw cause;
  }
}

/**
 * The request and the reading of its body, which the timeout covers as one.
 * A large page can send its headers at once and its body past the bound, and
 * mapped around `fetch` alone that timeout would reach the banner as the
 * browser's own untranslated sentence.
 */
async function exchange<T>(
  path: string,
  init: RequestInit,
  read: (response: Response) => Promise<T>,
): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, init);
  const requestId = res.headers?.get?.('x-request-id') ?? null;

  if (!res.ok) {
    const body = await res.text();
    // The envelope alone carries a message worth showing. A proxy's error page
    // is HTML, and an empty body has nothing to say: both leave the message
    // empty, and `describeError` words the status instead.
    let message = '';
    let kind = 'http_error';
    let confirm: string | null = null;
    let includes: string[] = [];
    try {
      const json = JSON.parse(body) as Partial<ErrorBody>;
      message = json.message ?? json.error ?? '';
      kind = json.error ?? kind;
      confirm = typeof json.confirm === 'string' ? json.confirm : null;
      includes = Array.isArray(json.includes) ? json.includes : [];
    } catch {
      // Not the envelope.
    }
    throw new ApiError(message, res.status, kind, requestId, confirm, includes);
  }

  // A proxy whose session ran out answers with its sign-in page: followed as a
  // redirect, or served as a 200 in place of the answer. Read as JSON, that
  // page surfaces as the browser's own parse error.
  const page = read !== readBlob && (res.headers?.get?.('content-type') ?? '').includes('html');
  if (res.redirected || page) throw new ApiError('', res.status, 'unexpected_answer', requestId);

  if (res.status === 204) return undefined as T;
  return read(res);
}

/**
 * A file the API serves, fetched rather than linked: a refusal reaches the
 * screen as any other does, where a link would save the error's body as the
 * file. It has the bound and the error reading of every other request.
 */
const download = (path: string) => request<Blob>(path, {}, readBlob);

/**
 * How long a followed job is left between two looks at it: soon at first, as
 * most finish in a moment, then no more than once a second.
 */
const FOLLOW_FIRST_MS = 200;
const FOLLOW_EVERY_MS = 1000;

/**
 * A write that can run past the request bound: an apply over a whole library,
 * a sync of a large one. Waited for, it would be cut at the bound and read as
 * a server that never answered, while the server carries on and the result is
 * never shown.
 *
 * Asked not to wait (`Prefer: respond-async`), the server answers once the
 * work has started its job, and the job is followed to the report it keeps.
 * A refusal made before the job starts, as a guardrail's question, still
 * answers the first request, so `answering` sees it as before. A server that
 * did the work at once answers the report itself.
 */
async function followed<T>(path: string, init: RequestInit, following: Following = {}): Promise<T> {
  const signal = following.signal instanceof AbortSignal ? following.signal : undefined;
  const first = await request<{ job?: string; report?: T }>(
    path,
    {
      ...init,
      signal,
      headers: { ...(init.headers as Record<string, string>), Prefer: 'respond-async' },
    },
    async (response) =>
      response.status === 202
        ? { job: ((await response.json()) as { job_id: string }).job_id }
        : { report: (await response.json()) as T },
  );
  if (first.job === undefined) return first.report as T;
  await pause(FOLLOW_FIRST_MS, signal);
  return followJob<T>(first.job, following);
}

/** What a caller following a job hears of it, and how it stops looking. */
export interface Following {
  /** Each look at the job while it runs, for a screen showing how far it has gone. */
  onProgress?: (job: Job) => void;
  /**
   * Stops the looking, not the work: the job runs on, and a screen opened
   * again finds it under `/jobs` and follows it from there.
   */
  signal?: AbortSignal;
}

/** `ms` of quiet, cut short by `signal`. */
function pause(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(signal.reason as Error);
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      'abort',
      () => {
        clearTimeout(timer);
        reject(signal.reason as Error);
      },
      { once: true },
    );
  });
}

/** A started job, looked at until it finishes, and the report it keeps. */
async function followJob<T>(id: string, following: Following = {}): Promise<T> {
  const signal = following.signal instanceof AbortSignal ? following.signal : undefined;
  for (let wait = FOLLOW_FIRST_MS; ; wait = Math.min(wait * 2, FOLLOW_EVERY_MS)) {
    const job = await request<Job>(`/jobs/${id}`, { signal });
    if (job.status === 'running') {
      if (typeof following.onProgress === 'function') following.onProgress(job);
      await pause(wait, signal);
      continue;
    }
    // Every move of an apply refused fails the job and still keeps its report,
    // which says why move by move.
    if (job.result !== null && job.result !== undefined) return job.result as T;
    throw new ApiError(job.error_message ?? '', 0, 'job_failed');
  }
}

type QueryParams = Record<string, string | number | boolean | undefined>;

const body = (data: unknown) => JSON.stringify(data ?? {});
const query = (params?: QueryParams) => {
  if (!params) return '';
  const entries = Object.entries(params).filter(([, v]) => v !== undefined && v !== '');
  if (entries.length === 0) return '';
  return '?' + new URLSearchParams(entries.map(([k, v]) => [k, String(v)])).toString();
};

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
  probeInstance: (data: InstanceProbe, signal?: AbortSignal) =>
    request<TestConnectionResponse>('/instances/test', {
      method: 'POST',
      body: body(data),
      signal,
    }),
  syncInstance: (id: string) => followed<SyncReport>(`/instances/${id}/sync`, { method: 'POST' }),
  syncAll: () => followed<SyncReport[]>('/instances/sync', { method: 'POST' }),
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
    request<{ deleted: boolean }>(`/rule-tests/${id}`, { method: 'DELETE' }),
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
  /** The API key, sent once for a session cookie, in `apikey` mode. */
  keySession: (key: string) =>
    request<{ ok: boolean }>('/auth/key-session', { method: 'POST', body: body({ key }) }),
  /**
   * The new key comes back exactly once. No route reads it again, so a caller
   * that drops it has to mint another.
   */
  rotateApiKey: (proof: Proof) =>
    request<{ api_key: string }>('/auth/api-key', { method: 'POST', body: body(proof) }),
  deleteApiKey: (proof: Proof) =>
    request<unknown>('/auth/api-key', { method: 'DELETE', body: body(proof) }),
  changePassword: (current: string, newPassword: string, revokeKeys: boolean) =>
    request<PasswordChanged>('/auth/password', {
      method: 'PUT',
      body: body({ current, new_password: newPassword, revoke_keys: revokeKeys }),
    }),
  getSessions: (signal?: AbortSignal) => request<Session[]>('/auth/sessions', { signal }),
  endSession: (handle: string) =>
    request<unknown>(`/auth/sessions/${encodeURIComponent(handle)}`, { method: 'DELETE' }),
  endEverySession: () => request<{ ended: number }>('/auth/sessions', { method: 'DELETE' }),

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
  /** `replaceRules`: the bundle's rules replace the rules in place rather than join them. */
  importConfig: (bundle: unknown, replaceRules = false) =>
    request<ConfigImportReport>('/config/import', {
      method: 'POST',
      body: body({ bundle, replace_rules: replaceRules }),
    }),

  importRules: (bundle: RuleBundle, replace: boolean) =>
    request<{ imported: number; skipped: string[]; adjusted: string[] }>('/rules/import', {
      method: 'POST',
      body: body({ bundle, replace }),
    }),

  // ---------------------------------------------------------- media
  getMedia: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<MediaListItem>>(`/media${query(params)}`, { signal }),
  explainMedia: (id: string, signal?: AbortSignal) =>
    request<Explanation>(`/media/${id}/explain`, { signal }),

  // ---------------------------------------------------------- decisions
  /**
   * Followed, so a library of any size is never cut at the request bound. A
   * stored run's report keeps its counts alone: its proposals are listed
   * under `/decisions` by its `simulation_id`.
   */
  runSimulation: (data?: unknown, following?: Following) =>
    followed<SimulationResult>('/simulate', { method: 'POST', body: body(data) }, following),
  /** A job someone else started, followed to its report from the screen that shows it. */
  followJob: <T>(id: string, following?: Following) => followJob<T>(id, following),
  getDecisions: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<Decision>>(`/decisions${query(params)}`, { signal }),
  /** `confirm` names the guardrails already answered, not a blanket yes. */
  applyDecisions: (
    decision_ids: string[],
    move_files = false,
    confirm: string[] = [],
    following?: Following,
  ) =>
    followed<ApplyReport>(
      '/decisions/apply',
      { method: 'POST', body: body({ decision_ids, move_files, confirm }) },
      following,
    ),
  /** Apply everything one simulation proposed, in slices of `batch_limit`. */
  applyAllDecisions: (
    simulation_id: string,
    move_files = false,
    confirm: string[] = [],
    following?: Following,
  ) =>
    followed<BatchApplyReport>(
      '/decisions/apply-all',
      { method: 'POST', body: body({ simulation_id, move_files, confirm }) },
      following,
    ),

  revertDecisions: (decision_ids: string[], move_files = false, confirm: string[] = []) =>
    followed<ApplyReport>('/decisions/revert', {
      method: 'POST',
      body: body({ decision_ids, move_files, confirm }),
    }),
  /** Stop a running apply or revert before its next move. It answers before it stops. */
  cancelJob: (id: string) =>
    request<void>(`/jobs/${id}/cancel`, { method: 'POST' }, () => Promise.resolve()),

  // ---------------------------------------------------------- overrides
  /** The contract other applications call, as the running version describes it. */
  openApi: (signal?: AbortSignal) => request<OpenApiDocument>('/openapi.json', { signal }),

  webhookSigning: (signal?: AbortSignal) =>
    request<WebhookSigningStatus>('/notifications/webhook-secret', { signal }),
  rotateWebhookSigning: () =>
    request<{ secret: string }>('/notifications/webhook-secret', { method: 'POST' }),
  removeWebhookSigning: () =>
    request<unknown>('/notifications/webhook-secret', { method: 'DELETE' }),
  sendTestNotification: () => request<unknown>('/notifications/test', { method: 'POST' }),

  getApplications: (signal?: AbortSignal) => request<Application[]>('/applications', { signal }),
  createApplication: (data: NewApplication, proof: Proof) =>
    request<MintedApplication>('/applications', {
      method: 'POST',
      body: body({ ...data, ...proof }),
    }),
  revokeApplication: (id: string) => request<unknown>(`/applications/${id}`, { method: 'DELETE' }),

  getOverrides: (signal?: AbortSignal) => request<OverrideEntry[]>('/overrides', { signal }),
  createOverride: (data: unknown) =>
    request<OverrideEntry>('/overrides', { method: 'POST', body: body(data) }),
  deleteOverride: (id: string) => request<unknown>(`/overrides/${id}`, { method: 'DELETE' }),

  // ---------------------------------------------------------- logs
  getLogs: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<LogEntry>>(`/logs${query(params)}`, { signal }),
  /**
   * A link the browser follows itself: the sign-in has to leave this origin,
   * so it cannot be a fetch.
   */
  oidcStartUrl: () => `${API_BASE}/auth/oidc/start`,
  exportLogs: (params?: QueryParams) => download(`/logs/export${query(params)}`),
  getSecurityLog: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<SecurityEvent>>(`/security-log${query(params)}`, { signal }),
  exportSecurityLog: (params?: QueryParams) => download(`/security-log/export${query(params)}`),

  // ---------------------------------------------------------- settings
  getSettings: (signal?: AbortSignal) => request<StoredSettings>('/settings', { signal }),
  getMetadataProviders: (signal?: AbortSignal) =>
    request<MetadataProviders>('/metadata/providers', { signal }),
  listBackups: (signal?: AbortSignal) => request<BackupList>('/backups', { signal }),
  createBackup: () => request<BackupFile>('/backups', { method: 'POST', body: body({}) }),
  deleteBackup: (name: string) =>
    request<unknown>(`/backups/${encodeURIComponent(name)}`, { method: 'DELETE' }),
  /** `passphrase` opens a sealed archive, once the server asked for it. */
  restoreBackup: (name: string, passphrase?: string) =>
    request<RestoreResult>(`/backups/${encodeURIComponent(name)}/restore`, {
      method: 'POST',
      body: body(passphrase === undefined ? {} : { passphrase }),
    }),
  /** The archive carries the master key, so it is never reachable without one. */
  downloadBackup: (name: string) => download(`/backups/${encodeURIComponent(name)}`),
  updateSettings: (settings: Settings) =>
    request<unknown>('/settings', { method: 'PUT', body: body({ settings }) }),
  getOnboarding: (signal?: AbortSignal) => request<OnboardingStatus>('/onboarding', { signal }),
  setOnboarding: (state: OnboardingState) =>
    request<OnboardingStatus>('/onboarding', { method: 'PUT', body: body({ state }) }),
};

export interface RuleBundle {
  version: number;
  exported_at?: string | null;
  /** Each rule with the instances it is limited to, by name. */
  rules: (RuleDraft & { instance_names?: string[] | null })[];
  categories: string[];
}
