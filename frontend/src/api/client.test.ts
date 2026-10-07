import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { ApiError, api } from './client';
import { withBase } from '../test/base';

interface FakeResponse {
  ok?: boolean;
  status?: number;
  statusText?: string;
  /** Decoded payload. A string is returned verbatim by `text()`. */
  body?: unknown;
  headers?: Record<string, string>;
}

/** Stub `fetch` and hand back the spy so calls can be inspected. */
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
 * The URL and options of the nth `fetch`, or a failure that says so.
 *
 * Every assertion below reaches into `spy.mock.calls[0][1]`, and when the call
 * never happened that reads as `cannot read properties of undefined`, which
 * names neither the call that was expected nor the one that was made. Asserting
 * here turns the same mistake into "fetch was not called".
 */
function fetchCall(spy: ReturnType<typeof mockFetch>, index = 0) {
  const call = spy.mock.calls[index];
  if (!call) {
    throw new Error(
      `fetch was called ${spy.mock.calls.length} time(s), wanted at least ${index + 1}`,
    );
  }
  // `body` is narrowed to `string` here rather than left as `BodyInit`: this
  // client only ever sends JSON, and every assertion below parses it.
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
 * Every route the backend serves, as `VERB /path` with each parameter written
 * `{}`, read from the route table in `backend/src/main.rs`. Each `.route(`
 * call is read to the parenthesis that closes it, since a long one spans lines.
 */
const SERVED: Set<string> = (() => {
  const table = fs.readFileSync(
    path.resolve(__dirname, '..', '..', '..', 'backend', 'src', 'main.rs'),
    'utf-8',
  );
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

describe('the key in the browser', () => {
  it('sends no key header: the browser holds a session instead', async () => {
    const spy = mockFetch({ body: { status: 'ok' } });
    await api.getStatus();

    const headers = fetchCall(spy).options.headers;
    expect(headers['X-Api-Key']).toBeUndefined();
    expect(headers['Content-Type']).toBe('application/json');
  });

  /**
   * A key kept in the browser's storage is readable by any script on the
   * page. Found there, it goes once to the server for a session cookie, which
   * no script can read, and is removed before anything else is asked.
   */
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
    await api.getMedia({ search: '', media_type: undefined, page: 2 });

    expect(fetchCall(spy).url).toBe('/api/v1/media?page=2');
  });

  it('omits the question mark entirely when nothing is filtered', async () => {
    const spy = mockFetch({ body: { data: [], pagination: {} } });
    await api.getMedia({});

    expect(fetchCall(spy).url).toBe('/api/v1/media');
  });

  it('encodes values that would otherwise break the URL', async () => {
    const spy = mockFetch({ body: { data: [], pagination: {} } });
    await api.getMedia({ search: 'a&b=c d' });

    expect(fetchCall(spy).url).toBe('/api/v1/media?search=a%26b%3Dc+d');
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

  /**
   * A name, not a yes. Several guardrails can refuse the same apply, and a
   * boolean would answer every one of them at once.
   */
  it('names the guardrails already answered', async () => {
    const spy = mockFetch({ body: {} });
    await api.applyDecisions(['a', 'b'], true, ['capacity']);

    expect(JSON.parse(fetchCall(spy).options.body)).toEqual({
      decision_ids: ['a', 'b'],
      move_files: true,
      confirm: ['capacity'],
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

    const failure = await api.applyAllDecisions('s-1').catch((err: unknown) => err);
    expect((failure as ApiError).includes).toEqual(['unreachable']);
  });
});

/**
 * The whole surface, swept.
 *
 * `client.ts` is a long list of one-line methods that interpolate their
 * arguments into a path. The failure mode is not a type error but an argument
 * landing in the wrong hole, which produces `/instances/[object Object]` and a
 * 404 the caller reports as "not found". Nothing in the type system objects,
 * and no hand-written test covers every method.
 *
 * So every one of them is called with a placeholder and the request it made is
 * inspected: one call, to this API, with nothing unserialised in the path, and
 * a verb and a path the backend's route table serves.
 */
describe('every endpoint', () => {
  /** Methods that build a URL for the browser rather than fetching one. */
  // Files are fetched as blobs (`downloadBackup`, `exportLogs`), not linked.
  const URL_BUILDERS = ['oidcStartUrl'];

  const methods = Object.entries(api).filter(
    ([name, value]) => typeof value === 'function' && !URL_BUILDERS.includes(name),
  ) as [string, (...args: unknown[]) => unknown][];

  it('covers the whole client, so this sweep cannot quietly shrink', () => {
    // A floor rather than an exact count: the point is that the list is being
    // derived from the object, not that it has a particular size today.
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
  });

  it.each(methods)('%s issues one well-formed request', async (name, method) => {
    const spy = mockFetch({ body: {} });

    // Only the request is under test here, so a string standing in for an
    // object is harmless.
    await Promise.resolve(method(...PLACEHOLDERS)).catch(() => {});

    expect(spy, `${name} made no request`).toHaveBeenCalledTimes(1);
    const { url, options } = fetchCall(spy);
    expect(url, `${name} left the API base`).toContain('/api/v1/');
    expect(url, `${name} put an object in its path`).not.toContain('[object');
    expect(url, `${name} interpolated an absent argument`).not.toContain('undefined');
    // A verb or a path the server does not route answers 404 or 405, which the
    // caller reports as "not found" about something that exists.
    expect(SERVED, `${name} asks for a route the backend does not serve`).toContain(
      route(options.method ?? 'GET', url),
    );
  });

  it('builds the sign-in link without fetching it', () => {
    // A link the browser follows itself, off this origin and back. It must
    // still start at this API, at a route the server answers.
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
   * The browser's own timeout surfaces as a `TimeoutError`. It has to become an
   * `ApiError` with a stable kind, or the banner shows the DOMException's
   * English sentence whatever language the interface is in.
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
   * The timeout covers the body as well as the headers, and a large page can
   * send its headers at once and its body past the bound. Read outside the
   * mapping, that timeout would reach the banner as the browser's own
   * untranslated sentence, in every language.
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

  /** `fetch` as a browser implements it: pending until its signal aborts, then rejected with the reason. */
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
   * The point of handing a load a signal. A superseded or abandoned load has
   * to stop at the network, not only be ignored when it lands, and it has to
   * surface as a cancellation rather than as a timeout the banner would report.
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

  /** Where `AbortSignal.any` is missing, `anySignal` in `client.ts` composes by hand. */
  describe('in a browser that predates AbortSignal.any', () => {
    const any = AbortSignal.any;
    beforeEach(() => {
      Reflect.deleteProperty(AbortSignal, 'any');
    });
    afterEach(() => {
      Object.defineProperty(AbortSignal, 'any', { value: any, configurable: true, writable: true });
    });

    it("still aborts the request when the caller's signal aborts", async () => {
      stubPendingFetch();
      const caller = new AbortController();
      const pending = api.getStatus(caller.signal);
      caller.abort(new DOMException('superseded', 'AbortError'));

      const failure = await pending.catch((e: unknown) => e);
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

    it('still keeps the timeout when the caller passes a signal', async () => {
      stubPendingFetch();
      const timer = new AbortController();
      const timeout = vi.spyOn(AbortSignal, 'timeout').mockReturnValue(timer.signal);
      try {
        const pending = api.getStatus(new AbortController().signal);
        timer.abort(new DOMException('The operation timed out.', 'TimeoutError'));
        expect(vi.mocked(fetch).mock.calls[0]?.[1]?.signal?.aborted).toBe(true);

        const failure = await pending.catch((e: unknown) => e);
        expect(failure).toBeInstanceOf(ApiError);
        expect((failure as ApiError).kind).toBe('timeout');
      } finally {
        timeout.mockRestore();
      }
    });
  });

  /**
   * A caller's signal joins the timeout, it does not replace it. Replaced, the
   * request most likely to be waiting on a host that never answers would lose
   * its bound the moment it became cancellable.
   */
  it('keeps the timeout when the caller passes a signal of its own', async () => {
    stubPendingFetch();
    const timer = new AbortController();
    const timeout = vi.spyOn(AbortSignal, 'timeout').mockReturnValue(timer.signal);
    try {
      const pending = api.getStatus(new AbortController().signal);
      timer.abort(new DOMException('The operation timed out.', 'TimeoutError'));

      const failure = await pending.catch((e: unknown) => e);
      expect(failure).toBeInstanceOf(ApiError);
      expect((failure as ApiError).kind).toBe('timeout');
    } finally {
      timeout.mockRestore();
    }
  });

  /**
   * A refused connection is not a timeout, and saying so would send the user
   * looking in the wrong place. Left as it came, it is the browser's own
   * sentence, in the browser's language whatever the interface speaks.
   */
  it('reports a connection that failed as unreachable, not as a timeout', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));

    const failure = await api.getStatus().catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).kind).toBe('unreachable');
  });
});

describe('a failing response', () => {
  afterEach(() => vi.unstubAllGlobals());

  /**
   * The id is what an operator greps the log for. It has to survive the trip
   * from the header to the error the screen shows.
   */
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
   * A file is read as a blob, not as JSON, but its refusal is the same
   * envelope. Read as text, an archive pruned between the list and the click
   * shows the operator the literal `{"error":...}`.
   */
  it('reads the refusal of a download out of its envelope', async () => {
    mockFetch({ ok: false, status: 404, body: { error: 'not_found', message: 'No such backup' } });

    const failure = (await api.downloadBackup('gone.zip').catch((e: unknown) => e)) as ApiError;
    expect(failure.message).toBe('No such backup');
    expect(failure.kind).toBe('not_found');
  });
});

/**
 * The sign-in with a provider is a link the browser follows, so it carries the
 * mount point itself. The base is read once, as the module loads, which is why
 * this test loads a fresh copy under one.
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
  });
});

/**
 * A long write is followed through its job rather than waited for: cut at the
 * request bound, an apply over a whole library would read as a server that
 * never answered while the server finished it.
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
      .mockResolvedValueOnce(answer(202, { job_id: 'j1' }))
      .mockResolvedValueOnce(job('running'))
      .mockResolvedValueOnce(job('success', { result: report }));
    vi.stubGlobal('fetch', spy);

    const applied = api.applyDecisions(['d1']);
    await vi.advanceTimersByTimeAsync(2000);

    await expect(applied).resolves.toEqual(report);
    expect(fetchCall(spy).options.headers.Prefer).toBe('respond-async');
    expect(spy.mock.calls.slice(1).map(([url]) => url)).toEqual([
      '/api/v1/jobs/j1',
      '/api/v1/jobs/j1',
    ]);
  });

  it('tells the caller how far the job has gone, look by look', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { job_id: 'j1' }))
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
      .mockResolvedValueOnce(answer(202, { job_id: 'j1' }))
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
        .mockResolvedValueOnce(answer(202, { job_id: 'j1' }))
        .mockResolvedValueOnce(job('failed', { result: refused })),
    );

    const applied = api.applyAllDecisions('s1');
    await vi.advanceTimersByTimeAsync(1000);

    await expect(applied).resolves.toEqual(refused);
  });

  it('fails with what the job ran into', async () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(answer(202, { job_id: 'j1' }))
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

    const error = (await api.applyAllDecisions('s1').catch((e) => e)) as ApiError;
    expect(error.needsConfirmation).toBe(true);
    expect(error.confirm).toBe('batch');
  });
});
