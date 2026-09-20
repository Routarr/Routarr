import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, api, getApiKey, setApiKey } from './client';

interface FakeResponse {
  ok?: boolean;
  status?: number;
  statusText?: string;
  /** Decoded payload; a string is returned verbatim by `text()`. */
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
 * never happened that reads as `cannot read properties of undefined` — which
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
  localStorage.clear();
});

describe('api key storage', () => {
  it('round-trips through localStorage', () => {
    setApiKey('s3cret');
    expect(getApiKey()).toBe('s3cret');
  });

  it('trims and clears blank values', () => {
    setApiKey('  s3cret  ');
    expect(getApiKey()).toBe('s3cret');

    setApiKey('   ');
    expect(getApiKey()).toBe('');
  });
});

describe('request headers', () => {
  it('omits the key header when none is stored', async () => {
    const spy = mockFetch({ body: { status: 'ok' } });
    await api.getStatus();

    const headers = fetchCall(spy).options.headers;
    expect(headers['X-Api-Key']).toBeUndefined();
    expect(headers['Content-Type']).toBe('application/json');
  });

  it('sends the stored key on every call', async () => {
    setApiKey('s3cret');
    const spy = mockFetch({ body: { status: 'ok' } });
    await api.getStatus();

    expect(fetchCall(spy).options.headers['X-Api-Key']).toBe('s3cret');
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

  it('falls back to the raw text when the body is not our envelope', async () => {
    // A reverse proxy error page, for instance.
    mockFetch({ ok: false, status: 502, body: '<html>Bad Gateway</html>' });

    await expect(api.getRules()).rejects.toThrow('<html>Bad Gateway</html>');
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
   * boolean answered every one of them at once.
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
  });
});

/**
 * The whole surface, swept.
 *
 * `client.ts` is ninety one-line methods that interpolate their arguments into
 * a path. The failure mode is not a type error — it is an argument landing in
 * the wrong hole, which produces `/instances/[object Object]` and a 404 the
 * caller reports as "not found". Nothing in the type system objects, and no
 * hand-written test covers ninety methods.
 *
 * So every one of them is called with a placeholder and the request it made is
 * inspected: one call, to this API, with nothing unserialised in the path.
 */
describe('every endpoint', () => {
  /** Methods that build a URL for the browser rather than fetching one. */
  // The backup is fetched as a blob (`downloadBackup`), not linked: no URL builder for it.
  const URL_BUILDERS = ['logsExportUrl', 'oidcStartUrl'];

  const methods = Object.entries(api).filter(
    ([name, value]) => typeof value === 'function' && !URL_BUILDERS.includes(name),
  ) as [string, (...args: unknown[]) => unknown][];

  it('covers the whole client, so this sweep cannot quietly shrink', () => {
    // A floor rather than an exact count: the point is that the list is being
    // derived from the object, not that it has a particular size today.
    expect(methods.length).toBeGreaterThan(50);
  });

  it.each(methods)('%s issues one well-formed request', async (name, method) => {
    const spy = mockFetch({ body: {} });

    // Placeholders for whatever shape the method wants. Only the path is under
    // test here, so a string standing in for an object is harmless.
    await Promise.resolve(method('an-id', 'second', 'third')).catch(() => {});

    expect(spy, `${name} made no request`).toHaveBeenCalledTimes(1);
    const url = fetchCall(spy).url as string;
    expect(url, `${name} left the API base`).toContain('/api/v1/');
    expect(url, `${name} put an object in its path`).not.toContain('[object');
    expect(url, `${name} interpolated an absent argument`).not.toContain('undefined');
  });

  it('builds an export URL without fetching it', () => {
    // A plain link cannot carry the key header, so these hand a URL to code
    // that fetches it itself. They must still point at this API.
    for (const name of URL_BUILDERS) {
      const build = (api as unknown as Record<string, (...a: unknown[]) => string>)[name];
      // Named here and absent from the client, the loop skipped it silently.
      expect(typeof build, `${name} is not a URL builder on the client`).toBe('function');
      const url = (build as (...a: unknown[]) => string)({ search: 'akira' });
      expect(url, name).toContain('/api/v1/');
      expect(url, name).not.toContain('[object');
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

  /// Any other network failure keeps its own identity: a refused connection is
  /// not a timeout, and saying so would send the user looking in the wrong place.
  it('leaves other failures untouched', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));

    const failure = await api.getStatus().catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(TypeError);
  });
});

describe('a failing response', () => {
  afterEach(() => vi.unstubAllGlobals());

  /// The id is what an operator greps the log for; it has to survive the trip
  /// from the header to the error the screen shows.
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
});
