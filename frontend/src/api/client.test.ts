import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, EXPORTED_AT_MOST, api, onRefused } from './client';
import { withBase } from '../test/base';
import { backendConstant, readBackend } from '../test/backend';

interface FakeResponse {
  ok?: boolean;
  status?: number;
  statusText?: string;
  /** Decoded payload. A string is returned verbatim by `text()`. */
  body?: unknown;
  headers?: Record<string, string>;
}

function mockFetch(response: FakeResponse) {
  const payload = response.body ?? {};
  const spy = vi.fn().mockResolvedValue({
    ok: response.ok ?? true,
    status: response.status ?? 200,
    statusText: response.statusText ?? 'OK',
    headers: new Headers(response.headers ?? {}),
    json: async () => payload,
    text: async () => (typeof payload === 'string' ? payload : JSON.stringify(payload)),
  } as unknown as Response);
  vi.stubGlobal('fetch', spy);
  return spy;
}

/**
 * The URL and options of the nth `fetch`, or a failure that says so: indexed bare, a call that
 * never happened reads as `cannot read properties of undefined`, which names neither call.
 */
function fetchCall(spy: ReturnType<typeof mockFetch>, index = 0) {
  const call = spy.mock.calls[index];
  if (!call) {
    throw new Error(
      `fetch was called ${spy.mock.calls.length} time(s), wanted at least ${index + 1}`,
    );
  }
  // `body` is a string: the client sends only JSON, and every assertion below parses it.
  const [url, options] = call as [
    string,
    Omit<RequestInit, 'body'> & { headers: Record<string, string>; body: string },
  ];
  return { url, options };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

/**
 * Every route in `backend/src/main.rs`, as `VERB /path` with each parameter `{}`. Each `.route(`
 * call is read to the parenthesis that closes it, since a long one spans lines.
 */
const SERVED: Set<string> = (() => {
  const table = readBackend('backend/src/main.rs');
  const served = new Set<string>();
  for (const call of table.matchAll(/\.route\(/g)) {
    const start = (call.index ?? 0) + call[0].length;
    let end = start;
    for (let depth = 1; depth > 0 && end < table.length; end += 1) {
      if (table[end] === '(') depth += 1;
      else if (table[end] === ')') depth -= 1;
    }
    const args = table.slice(start, end);
    // The webhook route is a constant, and no client method calls it.
    const at = /^\s*"([^"]+)"/.exec(args)?.[1];
    if (!at) continue;
    for (const verb of args.matchAll(/\b(get|post|put|delete|patch)\(/g)) {
      served.add(`${verb[1]!.toUpperCase()} ${at.replace(/\{\w+\}/g, '{}')}`);
    }
  }
  return served;
})();

/** The deprecated paths `backend/src/api/deprecated.rs` answers, each parameter `{}`. */
const DEPRECATED: Set<string> = new Set(
  [...readBackend('backend/src/api/deprecated.rs').matchAll(/^\s*\("([^"]+)", "[^"]+"\),$/gm)].map(
    (row) => row[1]!.replace(/\{\w+\}/g, '{}'),
  ),
);

describe('the key in the browser', () => {
  it('reads an answer with no body as nothing, whatever its 2xx status', async () => {
    const spy = mockFetch({ status: 202, headers: { 'content-length': '0' } });
    spy.mockResolvedValueOnce({
      ok: true,
      status: 202,
      statusText: 'Accepted',
      headers: new Headers({ 'content-length': '0' }),
      json: () => Promise.reject(new SyntaxError('Unexpected end of JSON input')),
    } as unknown as Response);

    await expect(api.adoptCopyFolder()).resolves.toBeUndefined();
  });

  it('sends no key header: the browser holds a session instead', async () => {
    const spy = mockFetch({ body: { status: 'ok' } });
    await api.getStatus();

    const headers = fetchCall(spy).options.headers;
    expect(headers['X-Api-Key']).toBeUndefined();
    expect(headers['Content-Type']).toBe('application/json');
  });

  /** A key in storage is readable by any script on the page, a session cookie by none. */
  it('exchanges a key found in storage once for a session, and removes it', async () => {
    vi.resetModules();
    localStorage.setItem('routarr.apiKey', 's3cret');
    const spy = mockFetch({ body: { ok: true } });
    const fresh = await import('./client');

    await fresh.api.getStatus();
    await fresh.api.getStatus();

    expect(localStorage.getItem('routarr.apiKey')).toBeNull();
    expect(spy).toHaveBeenCalledTimes(3);
    const exchange = fetchCall(spy, 0);
    expect(exchange.url).toMatch(/\/api\/v1\/auth\/key-session$/);
    expect(JSON.parse(exchange.options.body as string)).toEqual({ key: 's3cret' });
    for (const later of [1, 2]) {
      expect(fetchCall(spy, later).url).toMatch(/\/status$/);
      expect(fetchCall(spy, later).options.headers['X-Api-Key']).toBeUndefined();
    }
  });
});

describe('error handling', () => {
  /** A refusal, whichever screen asked, shows the shell's gate at once, not at its next poll. */
  it('tells whoever listens of a refusal, and of nothing else', async () => {
    const refused = vi.fn();
    const stop = onRefused(refused);
    mockFetch({ ok: false, status: 404, body: { error: 'not_found', message: 'gone' } });
    await api.getRules().catch(() => {});
    expect(refused).not.toHaveBeenCalled();

    mockFetch({ ok: false, status: 401, body: { error: 'unauthorized', message: 'refused' } });
    await api.getRules().catch(() => {});
    expect(refused).toHaveBeenCalledTimes(1);

    stop();
    await api.getRules().catch(() => {});
    expect(refused).toHaveBeenCalledTimes(1);
  });

  it('surfaces the backend message, not the raw body', async () => {
    mockFetch({
      ok: false,
      status: 400,
      body: { error: 'bad_request', message: "Category 'nope' does not exist" },
    });

    await expect(api.getRules()).rejects.toThrow("Category 'nope' does not exist");
  });

  it('carries the status and error kind', async () => {
    mockFetch({
      ok: false,
      status: 404,
      body: { error: 'not_found', message: 'Rule x not found' },
    });

    const error = (await api.getRules().catch((e) => e)) as ApiError;
    expect(error).toBeInstanceOf(ApiError);
    expect(error.status).toBe(404);
    expect(error.kind).toBe('not_found');
  });

  /** A proxy's error page is HTML, and shown raw it fills every banner with markup. */
  it('keeps nothing of a body that is not the envelope', async () => {
    mockFetch({ ok: false, status: 502, body: '<html>Bad Gateway</html>' });

    const error = (await api.getRules().catch((e) => e)) as ApiError;
    expect(error).toMatchObject({ status: 502, kind: 'http_error', message: '' });
  });

  /** The header that makes several proxies answer 401 rather than redirect. */
  it('asks for JSON', async () => {
    const spy = mockFetch({ body: [], headers: { 'content-type': 'application/json' } });
    await api.getRules();

    expect(fetchCall(spy).options.headers.Accept).toBe('application/json');
  });

  it.each([
    ['followed to a sign-in page', { redirected: true }, 'application/json'],
    ['answered with a page in place of the answer', {}, 'text/html; charset=utf-8'],
  ])('takes an answer %s for what it is', async (_, extra, type) => {
    const spy = mockFetch({ body: '<html>Sign in</html>', headers: { 'content-type': type } });
    spy.mockResolvedValue({ ...(await spy.getMockImplementation()!()), ...extra });

    const error = (await api.getRules().catch((e) => e)) as ApiError;
    expect(error).toMatchObject({ kind: 'unexpected_answer' });
  });

  it('recognises the confirmation prompt so the UI can re-ask', async () => {
    mockFetch({
      ok: false,
      status: 409,
      body: {
        error: 'confirmation_required',
        message: '12 items exceed the threshold.',
      },
    });

    const error = (await api.applyDecisions(['a']).catch((e) => e)) as ApiError;
    expect(error.needsConfirmation).toBe(true);
  });

  it('does not mistake other conflicts for a confirmation prompt', async () => {
    mockFetch({
      ok: false,
      status: 409,
      body: { error: 'conflict', message: "Category 'anime' is already mapped to '/movies/anime'" },
    });

    const error = (await api.updateRootFolderCategory('rf-1', 'anime').catch((e) => e)) as ApiError;
    expect(error.needsConfirmation).toBe(false);
  });
});

describe('query building', () => {
  it('drops undefined and empty parameters', async () => {
    const spy = mockFetch({ body: { data: [], pagination: {} } });
    await api.getLibrary({ search: '', media_type: undefined, page: 2 });

    expect(fetchCall(spy).url).toBe('/api/v1/library?page=2');
  });

  it('omits the question mark entirely when nothing is filtered', async () => {
    const spy = mockFetch({ body: { data: [], pagination: {} } });
    await api.getLibrary({});

    expect(fetchCall(spy).url).toBe('/api/v1/library');
  });

  it('encodes values that would otherwise break the URL', async () => {
    const spy = mockFetch({ body: { data: [], pagination: {} } });
    await api.getLibrary({ search: 'a&b=c d' });

    expect(fetchCall(spy).url).toBe('/api/v1/library?search=a%26b%3Dc+d');
  });

  it('serialises booleans', async () => {
    const spy = mockFetch({ body: { data: [], pagination: {} } });
    await api.getDecisions({ include_superseded: true });

    expect(fetchCall(spy).url).toContain('include_superseded=true');
  });
});

describe('request bodies', () => {
  it('sends an empty object rather than nothing', async () => {
    const spy = mockFetch({ body: {} });
    await api.runSimulation();

    expect(fetchCall(spy).options.body).toBe('{}');
    expect(fetchCall(spy).options.method).toBe('POST');
  });

  /** A name, not a yes: a boolean would answer every guardrail that refuses the apply. */
  it('names the guardrails already answered', async () => {
    const spy = mockFetch({ body: {} });
    await api.applyDecisions(['a', 'b'], true, ['capacity']);

    expect(JSON.parse(fetchCall(spy).options.body)).toEqual({
      decision_ids: ['a', 'b'],
      move_files: true,
      confirm: ['capacity'],
    });
  });

  it('sends the filter of an apply or a revert of many beside its scope', async () => {
    const spy = mockFetch({ body: {} });
    await api.applyAllDecisions('s1', { category: 'anime' }, false, ['batch']);
    expect(JSON.parse(fetchCall(spy).options.body)).toEqual({
      simulation_id: 's1',
      filter: { category: 'anime' },
      move_files: false,
      confirm: ['batch'],
    });

    const scope = { filter: { rule_id: 'r1' }, since: '2026-10-18T00:00:00.000Z' };
    await api.revertAllDecisions(scope, true, ['batch']);
    expect(JSON.parse(fetchCall(spy, 1).options.body)).toEqual({
      ...scope,
      move_files: true,
      confirm: ['batch'],
    });
  });

  it('carries the name of the guardrail that is asking', async () => {
    mockFetch({
      ok: false,
      status: 409,
      body: { error: 'confirmation_required', message: 'No room', confirm: 'capacity' },
    });

    const failure = await api.applyDecisions(['a']).catch((err: unknown) => err);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).confirm).toBe('capacity');
    expect((failure as ApiError).includes).toEqual([]);
  });

  it('carries the other guardrails a question states', async () => {
    mockFetch({
      ok: false,
      status: 409,
      body: {
        error: 'confirmation_required',
        message: '2 items will move. The NAS is asleep.',
        confirm: 'batch',
        includes: ['unreachable'],
      },
    });

    const failure = await api.applyAllDecisions('s-1', {}).catch((err: unknown) => err);
    expect((failure as ApiError).includes).toEqual(['unreachable']);
  });
});

/**
 * Every method, called with placeholders. An argument in the wrong hole builds a path such as
 * `/instances/[object Object]`, a 404 no type check refuses, so each must make one request, to
 * this API, at a verb and a path the backend's route table serves.
 */
describe('every endpoint', () => {
  /** Methods that build a URL for the browser. Files are fetched as blobs, never linked. */
  const URL_BUILDERS = ['oidcStartUrl'];

  const methods = Object.entries(api).filter(
    ([name, value]) => typeof value === 'function' && !URL_BUILDERS.includes(name),
  ) as [string, (...args: unknown[]) => unknown][];

  it('covers the whole client, so this sweep cannot quietly shrink', () => {
    // A floor, not a count: the list is derived from the object, whatever its size.
    expect(methods.length).toBeGreaterThan(50);
  });

  /** Placeholders for whatever shape a method wants, each standing for one path parameter. */
  const PLACEHOLDERS = ['an-id', 'second', 'third'];

  /** A request as the route table writes it: `VERB /path`, every parameter `{}`, no query. */
  const route = (verb: string, url: string) =>
    `${verb} ${url
      .replace(/^.*\/api\/v1/, '')
      .replace(/\?.*$/, '')
      .split('/')
      .map((segment) => (PLACEHOLDERS.includes(decodeURIComponent(segment)) ? '{}' : segment))
      .join('/')}`;

  it('reads the backend route table rather than an empty one', () => {
    expect(SERVED.size).toBeGreaterThan(60);
    expect(SERVED).toContain('PUT /categories/{}');
    expect(DEPRECATED).toContain('/jobs/{}');
  });

  it.each(methods)('%s issues one well-formed request', async (name, method) => {
    const spy = mockFetch({ body: {} });

    // Only the request is under test, so a string standing in for an object is harmless.
    await Promise.resolve(method(...PLACEHOLDERS)).catch(() => {});

    expect(spy, `${name} made no request`).toHaveBeenCalledTimes(1);
    const { url, options } = fetchCall(spy);
    expect(url, `${name} left the API base`).toContain('/api/v1/');
    expect(url, `${name} put an object in its path`).not.toContain('[object');
    expect(url, `${name} interpolated an absent argument`).not.toContain('undefined');
    // An unrouted verb or path answers 404 or 405, reported as "not found" about what exists.
    const asked = route(options.method ?? 'GET', url);
    expect(SERVED, `${name} asks for a route the backend does not serve`).toContain(asked);
    expect(DEPRECATED, `${name} calls a deprecated path`).not.toContain(asked.split(' ')[1]);
  });

  it('builds the sign-in link without fetching it', () => {
    // Followed off this origin and back, it still starts at a route this API serves.
    for (const name of URL_BUILDERS) {
      const build = (api as unknown as Record<string, () => string>)[name];
      // A name listed here and absent from the client fails under its own name.
      expect(typeof build, `${name} is not a URL builder on the client`).toBe('function');
      const url = (build as () => string)();
      expect(url, name).toContain('/api/v1/');
      expect(SERVED, name).toContain(route('GET', url));
    }
  });
});

describe('a server that never answers', () => {
  afterEach(() => vi.unstubAllGlobals());

  /**
   * The browser's `TimeoutError` becomes an `ApiError` with a stable kind, or the banner shows
   * the DOMException's English sentence whatever language the interface speaks.
   */
  it('is reported as a timeout, not as a raw exception', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockRejectedValue(new DOMException('The operation timed out.', 'TimeoutError')),
    );

    const failure = await api.getStatus().catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).kind).toBe('timeout');
    expect((failure as ApiError).status).toBe(0);
  });

  /**
   * The timeout covers the body too: a large page can send its headers at once and its body past
   * the bound, and read outside the mapping it reaches the banner untranslated.
   */
  it.each([
    ['a reply', true, 200],
    ['a refusal', false, 409],
  ])('reports a timeout while reading the body of %s as a timeout', async (_what, ok, status) => {
    const late = () => Promise.reject(new DOMException('signal timed out', 'TimeoutError'));
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok,
        status,
        statusText: '',
        headers: new Headers(),
        json: late,
        text: late,
      }),
    );

    const failure = await api.getStatus().catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).kind).toBe('timeout');
  });

  it('bounds every request with a signal', async () => {
    const spy = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({}),
      text: async () => '{}',
    });
    vi.stubGlobal('fetch', spy);

    await api.getStatus();
    const init = spy.mock.calls[0]?.[1] as RequestInit | undefined;
    expect(init?.signal).toBeInstanceOf(AbortSignal);
  });

  /** A browser's `fetch`: pending until its signal aborts, then rejected with its reason. */
  function stubPendingFetch() {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (_url: string, init?: RequestInit) =>
          new Promise<Response>((_resolve, reject) => {
            const signal = init?.signal;
            if (signal?.aborted) return reject(signal.reason);
            signal?.addEventListener('abort', () => reject(signal.reason));
          }),
      ),
    );
  }

  /**
   * A superseded or abandoned load stops at the network rather than only being ignored, and
   * surfaces as a cancellation, not as a timeout the banner would report.
   */
  it("aborts the request when the caller's signal aborts", async () => {
    stubPendingFetch();
    const caller = new AbortController();
    const pending = api.getStatus(caller.signal);
    caller.abort(new DOMException('superseded', 'AbortError'));

    const failure = await pending.catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(DOMException);
    expect((failure as DOMException).name).toBe('AbortError');
  });

  it('fails at once for a caller whose signal has already aborted', async () => {
    stubPendingFetch();
    const caller = new AbortController();
    caller.abort(new DOMException('superseded', 'AbortError'));

    const failure = api.getStatus(caller.signal).catch((e: unknown) => e);
    // Aborted before the request leaves, not when the timeout comes round.
    expect(vi.mocked(fetch).mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
    expect(((await failure) as DOMException).name).toBe('AbortError');
  });

  /**
   * A caller's signal joins the timeout. Replaced, a request waiting on a host that never answers
   * would lose its bound the moment it became cancellable.
   */
  it('keeps the timeout when the caller passes a signal of its own', async () => {
    vi.useFakeTimers();
    try {
      stubPendingFetch();
      const pending = api.getStatus(new AbortController().signal).catch((e: unknown) => e);
      await vi.advanceTimersByTimeAsync(30_000);

      const failure = await pending;
      expect(failure).toBeInstanceOf(ApiError);
      expect((failure as ApiError).kind).toBe('timeout');
    } finally {
      vi.useRealTimers();
    }
  });

  /**
   * A refused connection is not a timeout, which would send the user looking elsewhere. Left as
   * it came, it is the browser's sentence, in the browser's language.
   */
  it('reports a connection that failed as unreachable, not as a timeout', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));

    const failure = await api.getStatus().catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).kind).toBe('unreachable');
  });
});

describe('a file the API serves', () => {
  afterEach(() => vi.useRealTimers());

  /**
   * A file's headers at once, then a chunk every `every` ms, `count` of them, then the end or a
   * stall. Aborted, it errors with the signal's reason, as a browser's body does.
   */
  function stubFile(
    count: number,
    every: number,
    end: 'close' | 'stall',
    headers: Record<string, string> = {},
  ) {
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init?: RequestInit) => {
        let over = false;
        const body = new ReadableStream<Uint8Array<ArrayBuffer>>({
          start(stream) {
            init?.signal?.addEventListener('abort', () => {
              if (!over) ((over = true), stream.error(init.signal?.reason));
            });
            const send = (left: number) => {
              if (left === 0) {
                if (end === 'close') ((over = true), stream.close());
                return;
              }
              setTimeout(() => {
                if (over) return;
                stream.enqueue(new Uint8Array(1024));
                send(left - 1);
              }, every);
            };
            send(count);
          },
        });
        const blob = async () => {
          const chunks: Uint8Array<ArrayBuffer>[] = [];
          for await (const chunk of body) chunks.push(chunk);
          return new Blob(chunks);
        };
        return { ok: true, status: 200, headers: new Headers(headers), body, blob } as Response;
      }),
    );
  }

  /** A large archive over a slow link takes as long as it takes, as long as it keeps arriving. */
  it('downloads a file that keeps arriving past the bound of one request', async () => {
    vi.useFakeTimers();
    stubFile(4, 10_000, 'close');

    const archive = api.downloadBackup('x.zip', {}).catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(45_000);

    expect(((await archive) as Blob).size).toBe(4 * 1024);
  });

  it('gives up on a file that stops arriving, once the bound passes', async () => {
    vi.useFakeTimers();
    stubFile(1, 1_000, 'stall');

    let settled = false;
    const archive = api
      .downloadBackup('x.zip', {})
      .catch((e: unknown) => e)
      .finally(() => (settled = true));
    // Thirty seconds after the last chunk, not after the request.
    await vi.advanceTimersByTimeAsync(30_500);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1_000);

    expect(await archive).toMatchObject({ kind: 'timeout' });
  });

  /** The bound restarts for a file's chunks only: a slow page of JSON is still cut. */
  it('keeps the bound of the whole request for a JSON body', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init?: RequestInit) => ({
        ok: true,
        status: 200,
        headers: new Headers({ 'content-type': 'application/json' }),
        json: () =>
          new Promise((_resolve, reject) =>
            init?.signal?.addEventListener('abort', () => reject(init.signal?.reason)),
          ),
      })),
    );

    const status = api.getStatus().catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(30_500);

    expect(await status).toMatchObject({ kind: 'timeout' });
  });

  /** An upload shows no progress to restart the bound on, and the identifier table is large. */
  it('gives a file sent as the body the time a slow uplink needs', async () => {
    vi.useFakeTimers();
    let settled = false;
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (_url: string, init?: RequestInit) =>
          new Promise<Response>((_resolve, reject) =>
            init?.signal?.addEventListener('abort', () => reject(init.signal?.reason)),
          ),
      ),
    );

    const sent = api
      .importAnimeIds(new File(['[]'], 'anime-ids.json'))
      .catch((e: unknown) => e)
      .finally(() => (settled = true));
    await vi.advanceTimersByTimeAsync(60_000);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(600_000);

    expect(await sent).toMatchObject({ kind: 'timeout' });
  });

  it('names the ceiling the server cuts a log export at', () => {
    expect(EXPORTED_AT_MOST).toBe(
      backendConstant('backend/src/api/move_log.rs', 'EXPORTED_AT_MOST'),
    );
  });

  it.each([
    ['exportMoveLog', () => api.exportMoveLog()],
    ['exportSecurityLog', () => api.exportSecurityLog()],
  ])('%s says when the server cut the file short', async (_name, exported) => {
    stubFile(1, 0, 'close', { 'x-routarr-truncated': 'true' });
    expect(await exported()).toMatchObject({ truncated: true });

    stubFile(1, 0, 'close');
    expect(await exported()).toMatchObject({ truncated: false });
  });
});

describe('a failing response', () => {
  afterEach(() => vi.unstubAllGlobals());

  /** The id an operator greps the log for survives from the header to the error shown. */
  it('carries the request id the server answered with', async () => {
    mockFetch({
      ok: false,
      status: 500,
      body: { error: 'database_error', message: 'An internal error occurred.' },
      headers: { 'x-request-id': 'req-8d1f' },
    });

    const failure = (await api.getStatus().catch((e: unknown) => e)) as ApiError;
    expect(failure.requestId).toBe('req-8d1f');
    expect(failure.status).toBe(500);
  });

  it('has no request id when the server sent none', async () => {
    mockFetch({ ok: false, status: 404, body: { error: 'not_found', message: 'No such rule' } });

    const failure = (await api.getStatus().catch((e: unknown) => e)) as ApiError;
    expect(failure.requestId).toBeNull();
  });

  /**
   * A file is read as a blob, but its refusal is the envelope. Read as text, an archive pruned
   * between the list and the click shows the literal `{"error":...}`.
   */
  it('reads the refusal of a download out of its envelope', async () => {
    mockFetch({ ok: false, status: 404, body: { error: 'not_found', message: 'No such backup' } });

    const failure = (await api.downloadBackup('gone.zip', {}).catch((e: unknown) => e)) as ApiError;
    expect(failure.message).toBe('No such backup');
    expect(failure.kind).toBe('not_found');
  });
});

/**
 * The provider sign-in is a link the browser follows, so it carries the mount point. The base is
 * read once as the module loads, so the test loads a fresh copy under one.
 */
describe('the provider sign-in link', () => {
  afterEach(() => {
    withBase(null);
    vi.resetModules();
  });

  it('keeps the mount point', async () => {
    withBase('/routarr/');
    vi.resetModules();
    const { api: mounted } = await import('./client');

    expect(mounted.oidcStartUrl()).toBe('/routarr/api/v1/auth/oidc/start');
    expect(mounted.oidcStartUrl('/history')).toBe(
      '/routarr/api/v1/auth/oidc/start?return=%2Fhistory',
    );
  });
});

/**
 * A long write is followed through its job: cut at the request bound, an apply over a whole
 * library would read as a server that never answered while it finished.
 */
describe('a write followed through its job', () => {
  const answer = (status: number, payload: unknown) => ({
    ok: status < 400,
    status,
    statusText: '',
    headers: new Headers({ 'content-type': 'application/json' }),
    json: async () => payload,
    text: async () => JSON.stringify(payload),
  });
  const job = (status: string, over: Record<string, unknown> = {}) =>
    answer(200, { id: 'j1', status, result: null, error_message: null, ...over });
  const report = { requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] };

  afterEach(() => vi.useRealTimers());

  it('asks not to wait, then reads the report the job kept', async () => {
    vi.useFakeTimers();
    const spy = vi
      .fn()
      .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
      .mockResolvedValueOnce(job('running'))
      .mockResolvedValueOnce(job('success', { result: report }));
    vi.stubGlobal('fetch', spy);

    const applied = api.applyDecisions(['d1']);
    await vi.advanceTimersByTimeAsync(2000);

    await expect(applied).resolves.toEqual(report);
    expect(fetchCall(spy).options.headers.Prefer).toBe('respond-async');
    expect(spy.mock.calls.slice(1).map(([url]) => url)).toEqual([
      '/api/v1/tasks/j1',
      '/api/v1/tasks/j1',
    ]);
  });

  /** A backup or a purge of a large library runs past the request bound. */
  it('follows a backup and a purge as tasks, past the bound of one request', async () => {
    for (const call of [() => api.createBackup(), () => api.purge()]) {
      const spy = vi.fn().mockResolvedValueOnce(answer(200, { name: 'taken' }));
      vi.stubGlobal('fetch', spy);

      await call();

      expect(fetchCall(spy).options.headers.Prefer).toBe('respond-async');
    }
  });

  it('tells the caller how far the job has gone, look by look', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
        .mockResolvedValueOnce(job('running', { progress_current: 5, progress_total: 10 }))
        .mockResolvedValueOnce(job('success', { result: { simulation_id: 's1' } })),
    );
    const seen: number[] = [];

    const run = api.runSimulation(
      { persist: true },
      { onProgress: (looked) => seen.push(looked.progress_current) },
    );
    await vi.advanceTimersByTimeAsync(2000);

    await expect(run).resolves.toEqual({ simulation_id: 's1' });
    expect(seen).toEqual([5]);
  });

  /** Leaving stops the looking, not the work, which a screen opened again finds. */
  it('stops looking once the caller leaves, and asks nothing more', async () => {
    vi.useFakeTimers();
    const spy = vi
      .fn()
      .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
      .mockResolvedValue(job('running'));
    vi.stubGlobal('fetch', spy);
    const leaving = new AbortController();

    const run = api.runSimulation({}, { signal: leaving.signal }).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(300);
    leaving.abort();
    const looks = spy.mock.calls.length;
    await vi.advanceTimersByTimeAsync(5000);

    expect(spy.mock.calls.length).toBe(looks);
    expect(await run).toMatchObject({ name: 'AbortError' });
  });

  /** A screen's signal lives as long as the screen, and a long task looks once a second. */
  it('takes back each wait it hung on the caller signal', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
        .mockResolvedValueOnce(job('running'))
        .mockResolvedValueOnce(job('running'))
        .mockResolvedValueOnce(job('running'))
        .mockResolvedValueOnce(job('success', { result: report })),
    );
    const leaving = new AbortController();
    const added = vi.spyOn(leaving.signal, 'addEventListener');
    const removed = vi.spyOn(leaving.signal, 'removeEventListener');
    const waits = () => added.mock.calls.filter(([type]) => type === 'abort').map(([, f]) => f);

    const applied = api.applyDecisions(['d1'], false, [], { signal: leaving.signal });
    await vi.advanceTimersByTimeAsync(10_000);

    await expect(applied).resolves.toEqual(report);
    expect(waits().length).toBeGreaterThan(3);
    const taken = removed.mock.calls.map(([, f]) => f);
    expect(waits().filter((listener) => !taken.includes(listener))).toEqual([]);
  });

  /** A proxy's hiccup during a long apply is not the apply failing. */
  it('looks again after a look that failed, and still reads the report', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
        .mockResolvedValueOnce(answer(502, { error: 'bad_gateway', message: '' }))
        .mockResolvedValueOnce(job('success', { result: report })),
    );

    const applied = api.applyDecisions(['d1']);
    await vi.advanceTimersByTimeAsync(10_000);

    await expect(applied).resolves.toEqual(report);
  });

  it('gives up once the looks keep failing', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
        .mockResolvedValue(answer(502, { error: 'bad_gateway', message: '' })),
    );

    const applied = api.applyDecisions(['d1']).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(60_000);

    expect(await applied).toMatchObject({ status: 502 });
  });

  /** Nobody reads a job followed in a hidden tab, and the server is left alone meanwhile. */
  it('waits while the tab is hidden, and looks again once it is back', async () => {
    vi.useFakeTimers();
    const spy = vi
      .fn()
      .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
      .mockResolvedValueOnce(job('running'))
      .mockResolvedValueOnce(job('success', { result: report }));
    vi.stubGlobal('fetch', spy);
    const hidden = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');

    const applied = api.applyDecisions(['d1']);
    await vi.advanceTimersByTimeAsync(300);
    hidden.mockReturnValue('hidden');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(spy).toHaveBeenCalledTimes(2);

    hidden.mockReturnValue('visible');
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(2000);

    await expect(applied).resolves.toEqual(report);
  });

  it('takes a report answered at once as it is', async () => {
    const spy = vi.fn().mockResolvedValueOnce(answer(200, report));
    vi.stubGlobal('fetch', spy);

    await expect(api.revertDecisions(['d1'])).resolves.toEqual(report);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('keeps the report of a job that failed on its own count', async () => {
    vi.useFakeTimers();
    const refused = { ...report, applied: 0, failed: 1 };
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
        .mockResolvedValueOnce(job('failed', { result: refused })),
    );

    const applied = api.applyAllDecisions('s1', {});
    await vi.advanceTimersByTimeAsync(1000);

    await expect(applied).resolves.toEqual(refused);
  });

  it('fails with what the job ran into', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { task_id: 'j1' }))
        .mockResolvedValueOnce(job('failed', { error_message: 'Radarr answered 401' })),
    );

    const synced = api.syncInstance('i1').catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(1000);

    expect(await synced).toMatchObject({ kind: 'job_failed', message: 'Radarr answered 401' });
  });

  it('hands a guardrail question back before any job starts', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValueOnce(
        answer(409, {
          error: 'confirmation_required',
          message: '12 items exceed the threshold.',
          confirm: 'batch',
        }),
      ),
    );

    const error = (await api.applyAllDecisions('s1', {}).catch((e) => e)) as ApiError;
    expect(error.needsConfirmation).toBe(true);
    expect(error.confirm).toBe('batch');
  });
});
