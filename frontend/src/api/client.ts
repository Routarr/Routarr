// Every request goes through `api` below. Nothing here imports Svelte or `frontend/src/lib/`.

import { basePath } from './basePath';
import type { OpenApiDocument } from './openapi';

import type {
  Application,
  AuthMode,
  ApplyReport,
  BatchApplyReport,
  ConfigImportReport,
  Category,
  EnrichmentReport,
  ConditionCatalog,
  Decision,
  DecisionFilter,
  ErrorBody,
  Explanation,
  KeepRequest,
  Me,
  Health,
  Instance,
  InstanceProbe,
  Job,
  LogEntry,
  MaintenanceReport,
  MappingConflict,
  FolderSuggestion,
  Dismissal,
  DeclaredRootFolder,
  Deleted,
  AnimeIdsState,
  AnimeIdsLoaded,
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
  RuleBundle,
  RuleDraft,
  RuleImportReport,
  RuleVerdict,
  SecurityEvent,
  Session,
  SigningSecret,
  RestoreResult,
  TemplateAdded,
  TemplatePreview,
  TemplateRequest,
  TemplateSummary,
  RulePreview,
  Settings,
  StoredSettings,
  SimulationResult,
  Status,
  WebhookSigningStatus,
  SyncReport,
  TestConnectionResponse,
  TitleRefresh,
  LibraryFacets,
  RuleHealthReport,
  RuleTest,
  RuleTestRun,
} from './types';

// Resolved once: the mount point cannot change while the page is loaded.
const API_BASE = `${basePath()}/api/v1`;

/**
 * A key a browser may still hold in `localStorage`, readable by any script on the page. Found
 * there, it is traded once for a session cookie, which no script can read, and removed.
 */
const STORED_KEY = 'routarr.apiKey';

/** Thrown for any non-2xx response, carrying the backend's message. */
export class ApiError extends Error {
  readonly status: number;
  readonly kind: string;
  /** The server's `X-Request-Id`, to find a failure in the log. Null if it never answered. */
  readonly requestId: string | null;
  /**
   * Which guardrail is asking, sent back to say what was looked at. A bare yes would answer all
   * three, and lift a batch threshold the operator was never shown.
   */
  readonly confirm: string | null;
  /** The other guardrails the question states, sent back with `confirm` or asked again. */
  readonly includes: string[];
  /** Why the server refused, beside `kind`: a running task, or a state to change. */
  readonly reason: string | null;

  constructor(
    message: string,
    status: number,
    kind: string,
    requestId: string | null = null,
    confirm: string | null = null,
    includes: string[] = [],
    reason: string | null = null,
  ) {
    super(message);
    this.status = status;
    this.kind = kind;
    this.requestId = requestId;
    this.confirm = confirm;
    this.includes = includes;
    this.reason = reason;
    this.name = 'ApiError';
  }

  get needsConfirmation(): boolean {
    // Keyed on the stable code: matching the message would break on any rewording.
    return this.kind === 'confirmation_required';
  }
}

let storedKeyRead = false;
let exchanging: Promise<void> | null = null;

/**
 * The exchange of a stored key, which every request made while it runs awaits. `null` once
 * there is nothing to wait for.
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

/** Short enough to be a signal. Work that may run longer is `followed` and never meets it. */
const REQUEST_TIMEOUT_MS = 30_000;

/** A file sent as the body shows no progress to restart the bound on, so it has a slow uplink's. */
const UPLOAD_TIMEOUT_MS = 600_000;

/** Told of every 401 from any screen, so the shell shows its gate at once. */
let refused: (() => void) | null = null;

/** Listen for refusals until the returned function is called. */
export function onRefused(listener: () => void): () => void {
  refused = listener;
  return () => {
    if (refused === listener) refused = null;
  };
}

/** Reads a body, calling `heard` as each part of it arrives. */
type Reader<T> = (response: Response, heard: () => void) => Promise<T>;

const readJson = (response: Response) => response.json() as Promise<never>;

/** The rows a log export holds at most, `EXPORTED_AT_MOST` in `backend/src/api/move_log.rs`. */
export const EXPORTED_AT_MOST = 50_000;

/** A file the API serves, and whether the server cut it short of what the filters hold. */
interface ServedFile {
  blob: Blob;
  truncated: boolean;
}

/**
 * A file, whatever its type: the one read held to no JSON. Each chunk restarts the bound, so a
 * large file keeps arriving for as long as it takes, and one that stalls is still given up.
 */
async function readFile(response: Response, heard: () => void): Promise<ServedFile> {
  const truncated = response.headers.get('x-routarr-truncated') === 'true';
  const reader = response.body?.getReader();
  if (!reader) return { blob: await response.blob(), truncated };
  const chunks: Uint8Array<ArrayBuffer>[] = [];
  for (let part = await reader.read(); !part.done; part = await reader.read()) {
    heard();
    chunks.push(part.value);
  }
  return {
    blob: new Blob(chunks, { type: response.headers.get('content-type') ?? '' }),
    truncated,
  };
}

/** Aborts once `ms` pass with nothing heard. `heard` starts the wait again. */
function bound(ms: number) {
  const controller = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const heard = () => {
    clearTimeout(timer);
    timer = setTimeout(
      () => controller.abort(new DOMException('The request timed out.', 'TimeoutError')),
      ms,
    );
  };
  heard();
  return { signal: controller.signal, heard, stop: () => clearTimeout(timer) };
}

async function request<T>(
  path: string,
  options: RequestInit = {},
  read: Reader<T> = readJson,
): Promise<T> {
  const pending = storedKeyExchange();
  if (pending) await pending;
  // `Accept` asks a proxy in front for a 401 rather than a redirect to its sign-in page.
  const headers: Record<string, string> = {
    Accept: 'application/json',
    'Content-Type': 'application/json',
    ...(options.headers as Record<string, string> | undefined),
  };

  // Composed with the caller's signal, so a cancellable request keeps its bound.
  const timeout = bound(options.body instanceof Blob ? UPLOAD_TIMEOUT_MS : REQUEST_TIMEOUT_MS);
  // The sweep in `client.test.ts` passes a string here, which `AbortSignal.any` throws on.
  const caller = options.signal instanceof AbortSignal ? options.signal : null;
  const signal = caller ? AbortSignal.any([caller, timeout.signal]) : timeout.signal;
  try {
    return await exchange<T>(path, { ...options, headers, signal }, read, timeout.heard);
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === 'TimeoutError') {
      throw new ApiError('', 0, 'timeout');
    }
    // A failed connection: its message is in the browser's language, not the interface's.
    if (cause instanceof TypeError) throw new ApiError('', 0, 'unreachable');
    throw cause;
  } finally {
    timeout.stop();
  }
}

/**
 * The request and the reading of its body, under one timeout: a body arriving past the bound
 * would otherwise reach the banner as the browser's untranslated sentence.
 */
async function exchange<T>(
  path: string,
  init: RequestInit,
  read: Reader<T>,
  heard: () => void = () => {},
): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, init);
  const requestId = res.headers?.get?.('x-request-id') ?? null;

  if (!res.ok) {
    const body = await res.text();
    // Only the envelope carries a message. Without it `describeError` words the status.
    let message = '';
    let kind = 'http_error';
    let confirm: string | null = null;
    let includes: string[] = [];
    let reason: string | null = null;
    try {
      const json = JSON.parse(body) as Partial<ErrorBody>;
      message = json.message ?? json.error ?? '';
      kind = json.error ?? kind;
      confirm = typeof json.confirm === 'string' ? json.confirm : null;
      includes = Array.isArray(json.includes) ? json.includes : [];
      reason = typeof json.reason === 'string' ? json.reason : null;
    } catch {
      // Not the envelope.
    }
    if (res.status === 401) refused?.();
    throw new ApiError(message, res.status, kind, requestId, confirm, includes, reason);
  }

  // A proxy whose session ran out answers its sign-in page, redirected or as a 200.
  const page = read !== readFile && (res.headers?.get?.('content-type') ?? '').includes('html');
  if (res.redirected || page) throw new ApiError('', res.status, 'unexpected_answer', requestId);

  // A 202 that only starts work, as taking the backup copy folder does, carries no body.
  if (res.status === 204 || res.headers?.get?.('content-length') === '0') return undefined as T;
  return read(res, heard);
}

/** Fetched rather than linked, so a refusal reaches the screen instead of saving as the file. */
const download = (path: string, init: RequestInit = {}) =>
  request<ServedFile>(path, init, readFile);

/** Between two looks at a job: soon at first, as most finish at once, then once a second. */
const FOLLOW_FIRST_MS = 200;
const FOLLOW_EVERY_MS = 1000;

/**
 * Failed looks in a row before the follow gives up. The job runs on regardless, and one missed
 * look reported as a failure would read as the apply failing while its moves complete.
 */
const FOLLOW_RETRIES = 3;

/**
 * A write that can run past the request bound, sent with `Prefer: respond-async` and followed
 * through its job. A refusal before the job starts, or work done at once, answers directly.
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
        ? { job: ((await response.json()) as { task_id: string }).task_id }
        : { report: (await response.json()) as T },
  );
  if (first.job === undefined) return first.report as T;
  await pause(FOLLOW_FIRST_MS, signal);
  return followTask<T>(first.job, following);
}

/** What a caller following a job hears of it, and how it stops looking. */
export interface Following {
  /** Each look at the job while it runs, for a screen showing how far it has gone. */
  onProgress?: (job: Job) => void;
  /** Stops the looking, not the job, which a screen opened again finds under `/tasks`. */
  signal?: AbortSignal;
}

/** `ms` of quiet, cut short by `signal`, whose listener goes with the wait. */
function pause(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(signal.reason as Error);
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason as Error);
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/** A failure the next look may not meet: no answer, or the server's own. */
const transient = (cause: unknown) =>
  cause instanceof ApiError &&
  (cause.kind === 'timeout' || cause.kind === 'unreachable' || cause.status >= 500);

/** Resolves once the tab is in front: following in a hidden tab loads the server for nobody. */
function inFront(signal?: AbortSignal): Promise<void> {
  if (document.visibilityState !== 'hidden') return Promise.resolve();
  return new Promise((resolve, reject) => {
    const settle = () => {
      document.removeEventListener('visibilitychange', onChange);
      signal?.removeEventListener('abort', onAbort);
    };
    const onChange = () => {
      if (document.visibilityState === 'hidden') return;
      settle();
      resolve();
    };
    const onAbort = () => {
      settle();
      reject(signal?.reason as Error);
    };
    document.addEventListener('visibilitychange', onChange);
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/** A started job, looked at until it finishes, and the report it keeps. */
async function followTask<T>(id: string, following: Following = {}): Promise<T> {
  const signal = following.signal instanceof AbortSignal ? following.signal : undefined;
  let failed = 0;
  for (let wait = FOLLOW_FIRST_MS; ; wait = Math.min(wait * 2, FOLLOW_EVERY_MS)) {
    await inFront(signal);
    let job: Job;
    try {
      job = await request<Job>(`/tasks/${id}`, { signal });
      failed = 0;
    } catch (cause) {
      if (!transient(cause) || failed === FOLLOW_RETRIES) throw cause;
      failed += 1;
      await pause(FOLLOW_EVERY_MS * 2 ** failed, signal);
      continue;
    }
    if (job.status === 'running') {
      if (typeof following.onProgress === 'function') following.onProgress(job);
      await pause(wait, signal);
      continue;
    }
    // An apply whose every move was refused fails, and its report still says why, move by move.
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
  getLocalization: () => request<Localization>('/localization'),
  getLanguages: (signal?: AbortSignal) =>
    request<{
      default: string;
      /** `completion` is the share of English keys translated, 0–100. */
      languages: { code: string; name: string; completion: number; direction: 'ltr' | 'rtl' }[];
    }>('/localization/languages', { signal }),
  getStatus: (signal?: AbortSignal) => request<Status>('/status', { signal }),
  /**
   * `probe: false` reads the database alone. `fresh` probes again, where a
   * probe of the last 30 seconds is otherwise answered again.
   */
  getHealth: (options: { probe?: boolean; fresh?: boolean } = {}, signal?: AbortSignal) =>
    request<Health>(
      `/health${query({ probe: options.probe === false ? false : undefined, fresh: options.fresh || undefined })}`,
      { signal },
    ),
  getTasks: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<Job>>(`/tasks${query(params)}`, { signal }),
  purge: (following?: Following) =>
    followed<MaintenanceReport>('/maintenance/purge', { method: 'POST' }, following),

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
  syncInstance: (id: string, following?: Following) =>
    followed<SyncReport>(`/instances/${id}/sync`, { method: 'POST' }, following),
  removeMissing: (id: string, following?: Following) =>
    followed<SyncReport>(`/instances/${id}/remove-missing`, { method: 'POST' }, following),
  syncAll: (following?: Following) =>
    followed<SyncReport[]>('/instances/sync', { method: 'POST' }, following),
  rotateWebhookToken: (id: string) =>
    request<Instance>(`/instances/${id}/webhook-token`, { method: 'POST' }),

  getRootFolders: (signal?: AbortSignal) => request<RootFolder[]>('/root-folders', { signal }),
  /** Declare a destination the instance does not report as a root folder. */
  declareRootFolder: (instance_id: string, path: string) =>
    request<DeclaredRootFolder>('/root-folders', {
      method: 'POST',
      body: body({ instance_id, path }),
    }),
  deleteRootFolder: (id: string) =>
    request<unknown>(`/root-folders/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  getMappingConflicts: (signal?: AbortSignal) =>
    request<MappingConflict[]>('/root-folders/conflicts', { signal }),
  getAnimeIds: (signal?: AbortSignal) => request<AnimeIdsState>('/sources/anime-ids', { signal }),
  downloadAnimeIds: (following?: Following) =>
    followed<AnimeIdsLoaded>('/sources/anime-ids/download', { method: 'POST' }, following),
  importAnimeIds: (file: File, following?: Following) =>
    followed<AnimeIdsLoaded>(
      '/sources/anime-ids/import',
      { method: 'POST', body: file, headers: { 'Content-Type': 'application/json' } },
      following,
    ),
  getFolderSuggestions: (signal?: AbortSignal) =>
    request<FolderSuggestion[]>('/root-folders/suggestions', { signal }),
  dismissFolderSuggestion: (id: string) =>
    request<Dismissal>(`/root-folders/${encodeURIComponent(id)}/dismissal`, { method: 'PUT' }),
  restoreFolderSuggestion: (id: string) =>
    request<Deleted>(`/root-folders/${encodeURIComponent(id)}/dismissal`, {
      method: 'DELETE',
    }),
  updateRootFolderCategory: (id: string, category: string | null, addToSet = false) =>
    request<unknown>(`/root-folders/${id}/category`, {
      method: 'PUT',
      body: body(addToSet ? { category, add_to_set: true } : { category }),
    }),

  getCategories: (signal?: AbortSignal) => request<Category[]>('/categories', { signal }),
  createCategory: (data: unknown) =>
    request<Category>('/categories', { method: 'POST', body: body(data) }),
  renameCategory: (id: string, name: string) =>
    request<Category>(`/categories/${id}`, { method: 'PUT', body: body({ name }) }),
  deleteCategory: (id: string) => request<unknown>(`/categories/${id}`, { method: 'DELETE' }),

  getRules: (signal?: AbortSignal) => request<Rule[]>('/rules', { signal }),

  getRuleHealth: (signal?: AbortSignal) => request<RuleHealthReport>('/rules/health', { signal }),
  getLibraryFacets: (signal?: AbortSignal) => request<LibraryFacets>('/library/facets', { signal }),

  getRuleTests: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<RuleTest>>(`/rule-tests${query(params)}`, { signal }),
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
  deleteRuleTest: (id: string) => request<Deleted>(`/rule-tests/${id}`, { method: 'DELETE' }),
  setRuleTestExpectation: (id: string, expectedCategory: string) =>
    request<RuleTest>(`/rule-tests/${id}/expected-category`, {
      method: 'PUT',
      body: body({ expected_category: expectedCategory }),
    }),
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
  getMe: (signal?: AbortSignal) => request<Me>('/auth/me', { signal }),
  login: (username: string, password: string) =>
    request<{ username: string }>('/auth/login', {
      method: 'POST',
      body: body({ username, password }),
    }),
  logout: () => request<{ ok: boolean }>('/auth/logout', { method: 'POST' }),
  /** The API key, sent once for a session cookie, in `apikey` mode. */
  keySession: (key: string) =>
    request<{ ok: boolean }>('/auth/key-session', { method: 'POST', body: body({ key }) }),
  /** The new key comes back exactly once: no route reads it again. */
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

  validateRule: (data: RuleDraft, signal?: AbortSignal) =>
    request<RuleVerdict>('/rules/validate', {
      method: 'POST',
      body: body(data),
      signal,
    }),
  previewRule: (rule: RuleDraft, ruleId?: string) =>
    request<RulePreview>('/rules/preview', {
      method: 'POST',
      body: body({ rule, rule_id: ruleId ?? null }),
    }),
  exportRules: () => request<RuleBundle>('/rules/export'),
  getTemplates: (signal?: AbortSignal) =>
    request<TemplateSummary[]>('/rules/templates', { signal }),
  previewTemplate: (id: string, data: TemplateRequest, signal?: AbortSignal) =>
    request<TemplatePreview>(`/rules/templates/${encodeURIComponent(id)}/preview`, {
      method: 'POST',
      body: body(data),
      signal,
    }),
  importTemplate: (id: string, data: TemplateRequest) =>
    request<TemplateAdded>(`/rules/templates/${encodeURIComponent(id)}/import`, {
      method: 'POST',
      body: body(data),
    }),

  /** Everything that cannot be regenerated from the Arrs. Never carries a key. */
  exportConfig: () => request<unknown>('/config/export'),
  /** `replaceRules`: the bundle's rules replace the rules in place rather than join them. */
  importConfig: (bundle: unknown, replaceRules = false) =>
    request<ConfigImportReport>('/config/import', {
      method: 'POST',
      body: body({ bundle, replace_rules: replaceRules }),
    }),

  importRules: (bundle: RuleBundle, replace: boolean) =>
    request<RuleImportReport>('/rules/import', {
      method: 'POST',
      body: body({ bundle, replace }),
    }),

  getLibrary: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<MediaListItem>>(`/library${query(params)}`, { signal }),
  /** How many titles `params` lists, read off a page of one. */
  countLibrary: async (params: QueryParams, signal?: AbortSignal) => {
    const page = await request<Paginated<MediaListItem>>(
      `/library${query({ ...params, per_page: 1 })}`,
      { signal },
    );
    return page.pagination.total;
  },
  explainTitle: (id: string, signal?: AbortSignal) =>
    request<Explanation>(`/library/${id}/explain`, { signal }),

  /** Pin a title to the category of the folder it is in, so no run proposes to move it. */
  keepTitle: (id: string, body: KeepRequest = {}) =>
    request<OverrideEntry>(`/library/${encodeURIComponent(id)}/keep`, {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  /**
   * Followed, so no library is cut at the request bound. A stored run's report keeps its counts
   * alone, its proposals listed under `/decisions` by `simulation_id`.
   */
  runSimulation: (data?: unknown, following?: Following) =>
    followed<SimulationResult>('/simulate', { method: 'POST', body: body(data) }, following),
  /** A job someone else started, followed to its report from the screen that shows it. */
  followTask: <T>(id: string, following?: Following) => followTask<T>(id, following),
  getDecisions: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<Decision>>(`/decisions${query(params)}`, { signal }),
  /** How many decisions `params` lists, read off a page of one. */
  countDecisions: async (params: QueryParams, signal?: AbortSignal) => {
    const page = await request<Paginated<Decision>>(
      `/decisions${query({ ...params, per_page: 1 })}`,
      { signal },
    );
    return page.pagination.total;
  },
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
  /** Apply what one simulation proposed and `filter` keeps, in slices of `batch_limit`. */
  applyAllDecisions: (
    simulation_id: string,
    filter: DecisionFilter,
    move_files = false,
    confirm: string[] = [],
    following?: Following,
  ) =>
    followed<BatchApplyReport>(
      '/decisions/apply-all',
      { method: 'POST', body: body({ simulation_id, filter, move_files, confirm }) },
      following,
    ),

  revertDecisions: (
    decision_ids: string[],
    move_files = false,
    confirm: string[] = [],
    following?: Following,
  ) =>
    followed<ApplyReport>(
      '/decisions/revert',
      { method: 'POST', body: body({ decision_ids, move_files, confirm }) },
      following,
    ),
  /** Undo every applied move `scope` matches, in slices of `batch_limit`. */
  revertAllDecisions: (
    scope: { filter: DecisionFilter; simulation_id?: string; since?: string; until?: string },
    move_files = false,
    confirm: string[] = [],
    following?: Following,
  ) =>
    followed<BatchApplyReport>(
      '/decisions/revert-all',
      { method: 'POST', body: body({ ...scope, move_files, confirm }) },
      following,
    ),
  /** Stop a running apply or revert before its next move. It answers before it stops. */
  cancelTask: (id: string) =>
    request<void>(`/tasks/${id}/cancel`, { method: 'POST' }, () => Promise.resolve()),

  /** The contract other applications call, as the running version describes it. */
  openApi: (signal?: AbortSignal) => request<OpenApiDocument>('/openapi.json', { signal }),

  webhookSigning: (signal?: AbortSignal) =>
    request<WebhookSigningStatus>('/notifications/webhook-secret', { signal }),
  rotateWebhookSigning: () =>
    request<SigningSecret>('/notifications/webhook-secret', { method: 'POST' }),
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

  getExceptions: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<OverrideEntry>>(`/exceptions${query(params)}`, { signal }),
  createException: (data: unknown) =>
    request<OverrideEntry>('/exceptions', { method: 'POST', body: body(data) }),
  deleteException: (id: string) => request<unknown>(`/exceptions/${id}`, { method: 'DELETE' }),

  getMoveLog: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<LogEntry>>(`/move-log${query(params)}`, { signal }),
  /** A link the browser follows, since the sign-in leaves this origin, back to `screen`. */
  oidcStartUrl: (screen?: string) =>
    `${API_BASE}/auth/oidc/start${screen ? `?return=${encodeURIComponent(screen)}` : ''}`,
  exportMoveLog: (params?: QueryParams) => download(`/move-log/export${query(params)}`),
  getSecurityLog: (params?: QueryParams, signal?: AbortSignal) =>
    request<Paginated<SecurityEvent>>(`/security-log${query(params)}`, { signal }),
  exportSecurityLog: (params?: QueryParams) => download(`/security-log/export${query(params)}`),

  getSettings: (signal?: AbortSignal) => request<StoredSettings>('/settings', { signal }),
  getSources: (signal?: AbortSignal) => request<MetadataProviders>('/sources', { signal }),
  /** Ask every source again about one title at once, its search matches forgotten. */
  refreshTitleMetadata: (mediaId: string, following?: Following) =>
    followed<TitleRefresh>(
      '/sources/refresh',
      { method: 'POST', body: body({ media_id: mediaId }) },
      following,
    ),
  /** Have a pass read every answer `source` gave again. */
  refreshSource: (source: string, following?: Following) =>
    followed<EnrichmentReport>(
      '/sources/refresh',
      { method: 'POST', body: body({ source }) },
      following,
    ),
  listBackups: (signal?: AbortSignal) => request<BackupList>('/backups', { signal }),
  createBackup: (following?: Following) =>
    followed<BackupFile>('/backups', { method: 'POST', body: body({}) }, following),
  deleteBackup: (name: string) =>
    request<unknown>(`/backups/${encodeURIComponent(name)}`, { method: 'DELETE' }),
  /** Take the copy folder as it is now, without the marker of its first copy, and copy. */
  adoptCopyFolder: () =>
    request<unknown>('/backups/copy/adopt', { method: 'POST', body: body({}) }),
  /** Set the backup passphrase, an empty one stopping the encryption. */
  setBackupPassphrase: (passphrase: string, proof: Proof) =>
    request<unknown>('/backups/passphrase', {
      method: 'PUT',
      body: body({ passphrase, ...proof }),
    }),
  /** `passphrase` opens a sealed archive, once the server asked for it. */
  restoreBackup: (name: string, passphrase?: string) =>
    request<RestoreResult>(`/backups/${encodeURIComponent(name)}/restore`, {
      method: 'POST',
      body: body(passphrase === undefined ? {} : { passphrase }),
    }),
  /** The archive carries the master key and the API key, so a session sends its proof. */
  downloadBackup: (name: string, proof: Proof) =>
    download(`/backups/${encodeURIComponent(name)}/download`, {
      method: 'POST',
      body: body(proof),
    }).then((file) => file.blob),
  /** Withdraw the restore staged for the next start. */
  withdrawRestore: () => request<unknown>('/backups/restore', { method: 'DELETE' }),
  deleteReplacedDatabase: () =>
    request<unknown>('/backups/replaced-database', { method: 'DELETE' }),
  updateSettings: (settings: Settings) =>
    request<unknown>('/settings', { method: 'PUT', body: body({ settings }) }),
  getOnboarding: (signal?: AbortSignal) => request<OnboardingStatus>('/onboarding', { signal }),
  setOnboarding: (state: OnboardingState) =>
    request<OnboardingStatus>('/onboarding', { method: 'PUT', body: body({ state }) }),
};
