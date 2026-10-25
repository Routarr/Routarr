import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';

import { ApiError } from '../api/client';
import { seedDictionary } from './i18n.svelte';
import { ENGLISH } from '../test/render';
import { describeError } from './async.svelte';
import { access, publishAccess } from './access.svelte';
import AsyncHarness from '../test/AsyncHarness.svelte';

seedDictionary({ ...ENGLISH });

describe('describeError', () => {
  it('translates a refused key rather than echoing the server', () => {
    expect(describeError(new ApiError('Unauthorized', 401, 'unauthorized'))).toBe(
      'Your sign-in ended, or the key was refused.',
    );
  });

  it('names a timeout by its kind, not by the exception it came from', () => {
    expect(describeError(new ApiError('', 0, 'timeout'))).toBe(
      'The server did not answer in time. Check that Routarr is running, then try again.',
    );
  });

  it('names a server it could not reach in the interface language', () => {
    expect(describeError(new ApiError('', 0, 'unreachable'))).toBe(
      'The server could not be reached. Check that Routarr is running, then try again.',
    );
  });

  it('words a refusal that says nothing of its own by its status', () => {
    expect(describeError(new ApiError('', 502, 'http_error'))).toBe(
      'The server answered 502 without saying why. A proxy in front of Routarr may have refused the request.',
    );
    expect(describeError(new ApiError('', 502, 'http_error', 'req-1'))).toBe(
      'The server answered 502 without saying why. A proxy in front of Routarr may have refused the request. (request req-1)',
    );
  });

  /** A followed job that failed without a message: the Tasks screen holds its detail. */
  it('sends a failed task with nothing to say to the Tasks screen', () => {
    expect(describeError(new ApiError('', 0, 'job_failed'))).toBe(
      'The task failed. The Tasks screen says what it ran into.',
    );
    expect(describeError(new ApiError('Radarr answered 401', 0, 'job_failed'))).toBe(
      'Radarr answered 401',
    );
  });

  it('says a sign-in page answered rather than showing a parse error', () => {
    expect(describeError(new ApiError('', 200, 'unexpected_answer'))).toBe(
      "The server answered with a page that is not Routarr's, often the sign-in of a proxy in front of it. Sign in again, then reload.",
    );
  });

  it('appends the request id to a server-side failure', () => {
    const failure = new ApiError('Radarr timed out', 502, 'external_api_error', 'req-8d1f');
    expect(describeError(failure)).toBe('Radarr timed out (request req-8d1f)');
  });

  it('says a failure of the server in the interface language, with its id', () => {
    const failure = new ApiError('An internal error occurred.', 500, 'internal_error', 'req-9');
    expect(describeError(failure)).toBe(
      'Routarr failed on its side. The server log says why. (request req-9)',
    );
  });

  it('says a full disk and a busy database in the interface language', () => {
    const full = new ApiError('The disk is full.', 507, 'insufficient_storage', 'req-1');
    expect(describeError(full)).toBe(
      "The disk holding Routarr's data is full. Free some space, then try again. (request req-1)",
    );
    const busy = new ApiError('Busy.', 503, 'busy', null, null, [], 'database_busy');
    expect(describeError(busy)).toBe(
      'The database is busy with another write. Try again in a moment.',
    );
    expect(describeError(new ApiError('Checking a password.', 503, 'busy'))).toBe(
      'Checking a password.',
    );
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

  it('keeps an error through a reload of the same inputs, until the answer', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const answers = [first.promise, second.promise];
    const loader = vi.fn(() => answers.shift() ?? Promise.resolve('spare'));
    const { component } = render(AsyncHarness, { loader, filter: 'a' });
    first.reject(new ApiError('No such rule', 404, 'not_found'));
    await waitFor(() => expect(screen.getByTestId('error')).toHaveTextContent('No such rule'));

    void component.reload();
    await waitFor(() => expect(screen.getByTestId('loading')).toHaveTextContent('loading'));
    expect(screen.getByTestId('error')).toHaveTextContent('No such rule');

    second.resolve('found');
    await waitFor(() => expect(screen.getByTestId('data')).toHaveTextContent('found'));
    expect(screen.getByTestId('error')).toHaveTextContent('');
  });

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

  it('reports an abort it did not cause', async () => {
    const loader = vi.fn(() =>
      Promise.reject(new DOMException('The request was stopped', 'AbortError')),
    );
    render(AsyncHarness, { loader, filter: 'a' });

    await waitFor(() =>
      expect(screen.getByTestId('error')).toHaveTextContent('The request was stopped'),
    );
    expect(screen.getByTestId('loading')).toHaveTextContent('idle');
  });

  /** A load restarted then would cancel every request of the one in flight. */
  it('reloads on its inputs alone, not on what the loader reads before it awaits', async () => {
    publishAccess(null);
    const loader = vi.fn(() => Promise.resolve(access.readOnly ? 'reader' : 'owner'));
    render(AsyncHarness, { loader, filter: 'a' });
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(1));

    publishAccess(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(loader).toHaveBeenCalledTimes(1);
  });

  it('loads nothing once the component has gone away', async () => {
    const loader = vi.fn(() => Promise.resolve('data'));
    const { component, unmount } = render(AsyncHarness, { loader, filter: 'a' });
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(1));

    unmount();
    await component.reload();

    expect(loader).toHaveBeenCalledTimes(1);
  });

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
