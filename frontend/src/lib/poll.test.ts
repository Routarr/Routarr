import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/svelte';

import PollHarness from '../test/PollHarness.svelte';

/** Put the tab in front or behind, and tell the page it moved. */
function visibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
  document.dispatchEvent(new Event('visibilitychange'));
}

beforeEach(() => {
  vi.useFakeTimers();
  Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
});

afterEach(() => vi.useRealTimers());

describe('poll', () => {
  it('polls on the interval while the tab is in front', async () => {
    const reload = vi.fn();
    render(PollHarness, { reload, every: 1000, active: true });

    await vi.advanceTimersByTimeAsync(3500);

    expect(reload).toHaveBeenCalledTimes(3);
  });

  it('stops entirely while the tab is in the background', async () => {
    const reload = vi.fn();
    render(PollHarness, { reload, every: 1000, active: true });

    await vi.advanceTimersByTimeAsync(1000);
    expect(reload).toHaveBeenCalledTimes(1);

    visibility('hidden');
    await vi.advanceTimersByTimeAsync(60_000);

    expect(reload).toHaveBeenCalledTimes(1);
  });

  /** The data on screen is as stale as the time spent away. */
  it('catches up the moment the tab comes back, then resumes', async () => {
    const reload = vi.fn();
    render(PollHarness, { reload, every: 1000, active: true });

    visibility('hidden');
    await vi.advanceTimersByTimeAsync(60_000);
    expect(reload).toHaveBeenCalledTimes(0);

    visibility('visible');
    expect(reload).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1000);
    expect(reload).toHaveBeenCalledTimes(2);
  });

  it('does not poll at all while inactive', async () => {
    const reload = vi.fn();
    render(PollHarness, { reload, every: 1000, active: false });

    await vi.advanceTimersByTimeAsync(10_000);

    expect(reload).not.toHaveBeenCalled();
  });

  it('stops when the component goes away', async () => {
    const reload = vi.fn();
    const { unmount } = render(PollHarness, { reload, every: 1000, active: true });

    unmount();
    await vi.advanceTimersByTimeAsync(10_000);

    expect(reload).not.toHaveBeenCalled();
  });
});
