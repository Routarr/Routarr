import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { job, paginated } from '../test/fixtures';
import { api } from '../api/client';
import Jobs from './Jobs.svelte';

/**
 * Tasks is the window onto the only code that writes without anyone asking.
 * What it must get right is small: it refreshes while work is in flight and
 * stops when it is not, and it names a job by what it did rather than by the
 * identifier the database stores.
 */

const STRINGS = {
  Tasks: 'Tasks',
  AllStatuses: 'All statuses',
  FilterByStatus: 'Filter by status',
  AutoRefreshing: 'Refreshing',
  NoTaskYet: 'Nothing has run yet',
  JobSync: 'Library sync',
  TriggerScheduled: 'Scheduled',
  StatusRunning: 'Running',
  StatusSuccess: 'Succeeded',
  None: '—',
};

const show = () => renderWithI18n(Jobs, { strings: STRINGS });

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('Tasks', () => {
  it('names a job by what it did, not by the identifier it is stored under', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job()]));
    show();

    expect(await screen.findByText('Library sync')).toBeTruthy();
    // Scoped to the row: "Succeeded" is also an option in the status filter.
    const row = screen.getByText('Library sync').closest('tr') as HTMLElement;
    expect(within(row).getByText('Scheduled')).toBeTruthy();
    expect(within(row).getByText('Succeeded')).toBeTruthy();
  });

  it('says so when nothing has ever run, instead of showing an empty table', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([]));
    show();

    expect(await screen.findByText('Nothing has run yet')).toBeTruthy();
  });

  it('announces that it is refreshing while something is running', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job({ status: 'running' })]));
    show();

    expect(await screen.findByText('Refreshing')).toBeTruthy();
  });

  it('claims nothing about refreshing once everything has finished', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job({ status: 'success' })]));
    show();

    await screen.findByText('Library sync');
    expect(screen.queryByText('Refreshing')).toBeNull();
  });

  it('polls while a job runs, and stops once it finishes', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const getJobs = vi
      .spyOn(api, 'getJobs')
      .mockResolvedValue(paginated([job({ status: 'running' })]));
    show();

    await screen.findByText('Refreshing');
    const before = getJobs.mock.calls.length;

    await vi.advanceTimersByTimeAsync(3000);
    expect(getJobs.mock.calls.length).toBeGreaterThan(before);

    // Nothing running any more: the timer must go with it.
    getJobs.mockResolvedValue(paginated([job({ status: 'success' })]));
    await vi.advanceTimersByTimeAsync(3000);
    await waitFor(() => expect(screen.queryByText('Refreshing')).toBeNull());

    const settled = getJobs.mock.calls.length;
    await vi.advanceTimersByTimeAsync(30_000);
    expect(getJobs.mock.calls.length).toBe(settled);
  });

  it('asks the server for the status the user picked, rather than filtering on screen', async () => {
    const getJobs = vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job()]));
    show();
    await screen.findByText('Library sync');

    // `userEvent`, not a synthetic `change`: setting `.value` on a `<select>`
    // moves the DOM but not `selectedIndex`, so Svelte's binding never fires
    // and the filter silently does nothing — in the test only.
    await userEvent.selectOptions(screen.getByLabelText('Filter by status'), 'failed');

    await waitFor(() =>
      expect(getJobs).toHaveBeenCalledWith(expect.objectContaining({ status: 'failed' })),
    );
  });
});
