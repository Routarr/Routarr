import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';

import { ApiError } from '../api/client';
import { seedDictionary } from './i18n.svelte';
import { describeError } from './async.svelte';
import AsyncHarness from '../test/AsyncHarness.svelte';

/**
 * The one function every failure is worded by. What reaches the banner has to
 * be translated, has to say when the server was never reached, and has to hand
 * the operator the id that finds a server-side failure in the log — and only
 * then, since a refusal already says what to change.
 */

seedDictionary({
  Unauthorized: 'The key was refused',
  RequestTimedOut: 'The server did not answer in time',
  RequestId: 'request {id}',
});

describe('describeError', () => {
  it('translates a refused key rather than echoing the server', () => {
    expect(describeError(new ApiError('Unauthorized', 401, 'unauthorized'))).toBe(
      'The key was refused',
    );
  });

  it('names a timeout by its kind, not by the exception it came from', () => {
    expect(describeError(new ApiError('', 0, 'timeout'))).toBe('The server did not answer in time');
  });

  it('appends the request id to a server-side failure', () => {
    const failure = new ApiError('An internal error occurred.', 500, 'database_error', 'req-8d1f');
    expect(describeError(failure)).toBe('An internal error occurred. — request req-8d1f');
  });

  it('leaves a refusal alone even when it carries an id', () => {
    const refusal = new ApiError('No such rule', 404, 'not_found', 'req-0001');
    expect(describeError(refusal)).toBe('No such rule');
  });

  it('falls back to the message of anything else thrown', () => {
    expect(describeError(new Error('Failed to fetch'))).toBe('Failed to fetch');
    expect(describeError('odd')).toBe('odd');
  });
});

/** A promise the test resolves by hand, so two loads can finish out of order. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe('createAsync', () => {
  /**
   * The generation counter is the one thing here that is not a bare
   * try/catch, and nothing exercised it: a response arriving after the inputs
   * moved on must not overwrite the result of the newer request. Two loads,
   * the first answering last — the screen must show the second.
   */
  it('drops a response that arrives after the inputs moved on', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const answers = [first.promise, second.promise];
    const loader = vi.fn(() => answers.shift() ?? Promise.resolve('spare'));
    const { rerender } = render(AsyncHarness, { loader, filter: 'a' });
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(1));

    await rerender({ loader, filter: 'b' });
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(2));

    second.resolve('for b');
    await waitFor(() => expect(screen.getByTestId('data')).toHaveTextContent('for b'));
    first.resolve('for a');
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(screen.getByTestId('data')).toHaveTextContent('for b');
    expect(screen.getByTestId('loading')).toHaveTextContent('idle');
  });

  it('renders the failure as a message, and clears it on the next load', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const answers = [first.promise, second.promise];
    const loader = vi.fn(() => answers.shift() ?? Promise.resolve('spare'));
    const { rerender } = render(AsyncHarness, { loader, filter: 'a' });

    first.reject(new ApiError('No such rule', 404, 'not_found'));
    await waitFor(() => expect(screen.getByTestId('error')).toHaveTextContent('No such rule'));
    expect(screen.getByTestId('loading')).toHaveTextContent('idle');

    await rerender({ loader, filter: 'b' });
    await waitFor(() => expect(screen.getByTestId('loading')).toHaveTextContent('loading'));
    expect(screen.getByTestId('error')).toHaveTextContent('');
    second.resolve('fresh');
    await waitFor(() => expect(screen.getByTestId('data')).toHaveTextContent('fresh'));
  });

  /**
   * The generation counter stopped a late response overwriting a newer one,
   * but the request itself went on. A filter typed into quickly opened one
   * request per keystroke against somebody's own host, and every one of them
   * stayed in flight.
   */
  it('aborts the request a newer load supersedes', async () => {
    const signals: AbortSignal[] = [];
    const loader = vi.fn((signal: AbortSignal) => {
      signals.push(signal);
      return new Promise<string>(() => {});
    });
    const { rerender } = render(AsyncHarness, { loader, filter: 'a' });
    await waitFor(() => expect(signals).toHaveLength(1));
    expect(signals[0]?.aborted).toBe(false);

    await rerender({ loader, filter: 'b' });
    await waitFor(() => expect(signals).toHaveLength(2));

    expect(signals[0]?.aborted).toBe(true);
    expect((signals[0]?.reason as DOMException | undefined)?.name).toBe('AbortError');
    expect(signals[1]?.aborted).toBe(false);
  });

  /** A screen left mid-load must not hold its request open. */
  it('aborts the request in flight when the component goes away', async () => {
    const signals: AbortSignal[] = [];
    const loader = vi.fn((signal: AbortSignal) => {
      signals.push(signal);
      return new Promise<string>(() => {});
    });
    const { unmount } = render(AsyncHarness, { loader, filter: 'a' });
    await waitFor(() => expect(signals).toHaveLength(1));

    unmount();
    expect(signals[0]?.aborted).toBe(true);
  });
});
