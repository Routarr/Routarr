import { onDestroy } from 'svelte';

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
      return `${err.message} (${t('RequestId', { id: err.requestId })})`;
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
 * when the component goes away. The generation counter keeps a late response
 * off the screen, and the signal is what stops the request itself, which would
 * otherwise stay open against the operator's own host after nobody wants it. A
 * loader that ignores the argument still works, and simply cannot be cancelled.
 *
 * A cancellation it caused itself is not a failure and is not reported as one.
 * An abort from anywhere else is.
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
  let destroyed = false;

  async function reload() {
    // An action that finishes after its screen closed still calls this, and
    // nothing would ever cancel what it started.
    if (destroyed) return;
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
      // Silent only for a run this helper cancelled itself: a superseded run is
      // already discarded by the generation, and on teardown there is no
      // component left to show it in. An abort from anywhere else is a load
      // that did not happen, and reporting it as nothing would leave an empty
      // table with no banner.
      if (current === generation && !controller.signal.aborted) {
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
  });

  // `onDestroy` rather than the effect's own cleanup, which also runs before
  // every re-run on a `deps` change: marked destroyed there, a screen would stop
  // loading the first time a filter moved.
  onDestroy(() => {
    destroyed = true;
    inFlight?.abort(new DOMException('unmounted', 'AbortError'));
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
