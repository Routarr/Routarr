import { onDestroy } from 'svelte';

import { ApiError } from '../api/client';
import { t } from './i18n.svelte';

/** Turn anything thrown by the client into a message worth showing. */
export function describeError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 401) return t('Unauthorized');
    if (err.kind === 'timeout') return t('RequestTimedOut');
    if (err.kind === 'unreachable') return t('ServerUnreachable');
    if (err.kind === 'unexpected_answer') return t('UnexpectedAnswer');
    if (err.kind === 'job_failed' && !err.message) return t('TaskFailedSeeTasks');
    // A refusal with nothing to say of its own, as a proxy's error page.
    if (!err.message) {
      const said = t('ServerAnsweredStatus', { status: err.status });
      return err.requestId ? `${said} (${t('RequestId', { id: err.requestId })})` : said;
    }
    // The server says a failure of its own in English: a panic answers before
    // any setting is read.
    const own: Record<string, string> = {
      internal_error: 'InternalError',
      insufficient_storage: 'ErrorDiskFull',
    };
    const key = err.reason === 'database_busy' ? 'ErrorDatabaseBusy' : own[err.kind];
    const message = key ? t(key) : err.message;
    // A server-side failure is one the operator will look for in the log, and
    // the id is what finds it. A refusal (4xx) already says what to change.
    if (err.status >= 500 && err.requestId) {
      return `${message} (${t('RequestId', { id: err.requestId })})`;
    }
    return message;
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

interface Async<T> {
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
 * Load on mount, again when `deps` change and on `reload()`, with loading and error state. The
 * loader gets an `AbortSignal`, aborted when a newer run supersedes it or the component goes,
 * and a generation counter keeps a late answer off the screen. A cancellation it caused itself
 * is no failure, an abort from elsewhere is. Call it during component initialisation: it opens
 * an effect.
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

  /**
   * `fresh` for new inputs, whose last error was about something else. The
   * same inputs asked again keep it until the answer: cleared on the way, a
   * banner blinks at every poll, and a gate driven by it unmounts, taking what
   * was typed into it.
   */
  async function load(fresh: boolean) {
    // An action that finishes after its screen closed still calls this, and
    // nothing would ever cancel what it started.
    if (destroyed) return;
    inFlight?.abort(new DOMException('superseded', 'AbortError'));
    const controller = new AbortController();
    inFlight = controller;
    const current = ++generation;
    state.loading = true;
    if (fresh) {
      state.error = null;
      state.failure = null;
    }
    try {
      const result = await loader(controller.signal);
      if (current === generation) {
        state.data = result;
        state.error = null;
        state.failure = null;
      }
    } catch (err) {
      // Silent only for a run this helper cancelled itself. An abort from elsewhere is a load
      // that did not happen, which an empty table with no banner would hide.
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
  // change re-runs the loader: a dependency contract without a dependency
  // array. With no `deps` the effect runs once, which is the load-on-mount
  // case.
  $effect(() => {
    deps?.();
    void load(true);
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
    reload: () => load(false),
  };
}
