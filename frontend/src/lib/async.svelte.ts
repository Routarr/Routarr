import { ApiError } from '../api/client';
import { t } from './i18n.svelte';

/** Turn anything thrown by the client into a message worth showing. */
export function describeError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 401) return t('Unauthorized');
    if (err.kind === 'timeout') return t('RequestTimedOut');
    // A server-side failure is one the operator will look for in the log; the
    // id is what finds it. A refusal (4xx) already says what to change.
    if (err.status >= 500 && err.requestId) {
      return `${err.message} — ${t('RequestId', { id: err.requestId })}`;
    }
    return err.message;
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

export interface Async<T> {
  readonly data: T | null;
  readonly loading: boolean;
  /** The rendered message, for a banner. */
  error: string | null;
  /**
   * What was actually thrown. The message is enough for a banner but not for
   * deciding what to show *instead* of the page: a 401 needs a key prompt, not
   * a red line.
   */
  readonly failure: unknown;
  reload: () => Promise<void>;
}

/**
 * Load on mount and on demand, with loading and error state.
 *
 * One place for it, so a page cannot reimplement it as a bare try/catch that
 * only logs to the console and makes a failing request look like an empty
 * table.
 *
 * `loader` is read when it is called and nothing here is memoised, so there is
 * no identity to key an effect on and nothing to hold in a ref. What the
 * implementation does carry is a generation counter — a response that arrives
 * after the inputs moved on must not overwrite the result of a newer one.
 *
 * It is handed an `AbortSignal`, aborted when a newer run supersedes it and
 * when the component goes away. The generation counter already stopped a late
 * response from overwriting a newer one, but the request itself went on: a
 * filter typed into quickly opened one request per keystroke against somebody's
 * own host, and navigating away left every one of them in flight. A loader that
 * ignores the argument still works, and simply cannot be cancelled.
 *
 * A cancellation is not a failure, and is not reported as one. Supersession is
 * already covered by the generation counter, which discards the whole outcome
 * of a run a newer one replaced; what the filter below adds is the **teardown**
 * case, where the run is still the current generation and there is no longer a
 * component to render a banner into. That is deliberately not asserted by a
 * test: nothing observable distinguishes it, and a test written against it
 * passes with the filter removed.
 *
 * Call it during component initialisation: it opens an effect, which is what
 * both the first load and the reload-on-`deps` depend on.
 */
export function createAsync<T>(
  loader: (signal: AbortSignal) => Promise<T>,
  deps?: () => unknown,
): Async<T> {
  const state = $state({
    data: null as T | null,
    loading: true,
    error: null as string | null,
    failure: null as unknown,
  });

  let generation = 0;
  let inFlight: AbortController | null = null;

  /**
   * A cancellation this helper caused, which is not something to report.
   *
   * Matched on the exception rather than on `signal.aborted`, because a request
   * can fail for its own reasons in the same turn that a newer run starts, and
   * the operator is owed that error.
   */
  const cancelled = (err: unknown) => err instanceof DOMException && err.name === 'AbortError';

  async function reload() {
    inFlight?.abort(new DOMException('superseded', 'AbortError'));
    const controller = new AbortController();
    inFlight = controller;
    const current = ++generation;
    state.loading = true;
    state.error = null;
    state.failure = null;
    try {
      const result = await loader(controller.signal);
      if (current === generation) state.data = result;
    } catch (err) {
      if (current === generation && !cancelled(err)) {
        state.error = describeError(err);
        state.failure = err;
      }
    } finally {
      if (current === generation) {
        state.loading = false;
        inFlight = null;
      }
    }
  }

  // Reading `deps()` inside the effect is what subscribes to it, so a filter
  // change re-runs the loader — a dependency contract without a dependency
  // array. With no `deps` the effect runs once, which is the load-on-mount
  // case.
  $effect(() => {
    deps?.();
    void reload();
    // Teardown, not just supersession: a screen left mid-load would otherwise
    // hold its request open against a machine that is also running Radarr.
    return () => inFlight?.abort(new DOMException('unmounted', 'AbortError'));
  });

  return {
    get data() {
      return state.data;
    },
    get loading() {
      return state.loading;
    },
    get error() {
      return state.error;
    },
    set error(value: string | null) {
      state.error = value;
    },
    get failure() {
      return state.failure;
    },
    reload,
  };
}
