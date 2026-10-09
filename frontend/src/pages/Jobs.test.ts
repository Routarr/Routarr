import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { job, paginated } from '../test/fixtures';
import { nthCall } from '../test/spy';
import { api } from '../api/client';
import Jobs from './Jobs.svelte';
import { answerConfirmation } from '../test/confirm';

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
  JobSyncAll: 'Every instance synced',
  TriggerSchedule: 'Scheduled',
  TriggerApi: 'application',
  StatusRunning: 'Running',
  StatusSuccess: 'Succeeded',
  StatusCancelled: 'Stopped',
  JobApply: 'Apply',
  StopTask: 'Stop',
  ConfirmStopTask: 'Stop this task?',
  TaskStopRequested: 'Stop requested',
  None: '-',
  Next: 'Next',
  TaskCount: 'Tasks: {count}',
};

const show = () => renderWithI18n(Jobs, { strings: STRINGS });

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('Tasks', () => {
  /** A running apply can be stopped from here, after a question, and nothing else can. */
  it('stops a running apply once asked, and reads a stopped task as stopped', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(
      paginated([
        job({ id: 'j-apply', kind: 'apply', status: 'running' }),
        job({ id: 'j-sync', kind: 'sync', status: 'running' }),
        job({ id: 'j-old', kind: 'apply', status: 'cancelled' }),
      ]),
    );
    const cancelJob = vi.spyOn(api, 'cancelJob').mockResolvedValue(undefined);
    show();

    const stops = await screen.findAllByRole('button', { name: /^Stop/ });
    expect(stops).toHaveLength(1);
    expect(screen.getByText('Stopped', { selector: '.badge' })).toBeTruthy();
    await userEvent.click(stops[0]!);
    await answerConfirmation();

    await waitFor(() => expect(cancelJob).toHaveBeenCalledWith('j-apply'));
    expect(await screen.findByText('Stop requested')).toBeTruthy();
  });

  /** A kind of two words reads its own label, not its raw name. */
  it('names a task whose kind has two words', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job({ kind: 'sync_all' })]));
    show();

    expect(await screen.findByText('Every instance synced')).toBeTruthy();
  });

  /** Fifty tasks are half a day of syncs, and last night's failure sits on a later page. */
  it('reaches the tasks past the first page, and goes back to it on a filter', async () => {
    const getJobs = vi
      .spyOn(api, 'getJobs')
      .mockResolvedValue(paginated([job()], { total: 120, total_pages: 3 }));
    show();

    await userEvent.click(await screen.findByRole('button', { name: 'Next' }));
    await waitFor(() => expect(getJobs).toHaveBeenCalledTimes(2));
    expect(nthCall(getJobs, 1)[0]).toMatchObject({ page: 2, per_page: 50 });
    expect(screen.getByText(/Tasks: 120/)).toBeTruthy();

    await userEvent.selectOptions(screen.getByLabelText('Filter by status'), 'running');
    await waitFor(() => expect(getJobs).toHaveBeenCalledTimes(3));
    expect(nthCall(getJobs, 2)[0]).toMatchObject({ status: 'running', page: 1 });
  });

  it('names a job by what it did, not by the identifier it is stored under', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job()]));
    show();

    expect(await screen.findByText('Library sync')).toBeTruthy();
    // Scoped to the row: "Succeeded" is also an option in the status filter.
    const row = screen.getByText('Library sync').closest('tr') as HTMLElement;
    expect(within(row).getByText('Scheduled')).toBeTruthy();
    expect(within(row).getByText('Succeeded')).toBeTruthy();
  });

  /** An application's key names it, and the task says which one asked. */
  it('names the application that started a task beside its trigger', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(
      paginated([job({ trigger: 'api', subject: 'n8n' })]),
    );
    show();

    const row = (await screen.findByText('Library sync')).closest('tr') as HTMLElement;
    expect(within(row).getByText('application')).toBeTruthy();
    expect(within(row).getByText('n8n')).toBeTruthy();
  });

  /** Every move of an apply refused is a failure the task counts itself. */
  it('marks the detail of a task that failed on its own count as a failure', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(
      paginated([
        job({ status: 'failed', detail: 'Applied: 0, failed: 3', error_message: null }),
        job({ id: 'job-2', detail: 'Applied: 2, failed: 0' }),
      ]),
    );
    show();

    expect((await screen.findByText('Applied: 0, failed: 3')).className).toContain('text-danger');
    expect(screen.getByText('Applied: 2, failed: 0').className).toContain('text-muted');
  });

  it('says so when nothing has ever run, instead of showing an empty table', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([]));
    show();

    expect(await screen.findByText('Nothing has run yet')).toBeTruthy();
  });

  it('claims nothing about refreshing once everything has finished', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job({ status: 'success' })]));
    show();

    await screen.findByText('Library sync');
    expect(screen.queryByText('Refreshing')).toBeNull();
  });

  /**
   * Fast while a task runs, slow once nothing does: the scheduler starts a
   * sync on its own, and a screen read only while something already ran would
   * never show it.
   */
  it('polls fast while a task runs, then at an idle pace', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const getJobs = vi
      .spyOn(api, 'getJobs')
      .mockResolvedValue(paginated([job({ status: 'running' })]));
    show();

    await screen.findByText('Refreshing');
    const before = getJobs.mock.calls.length;

    await vi.advanceTimersByTimeAsync(3000);
    expect(getJobs.mock.calls.length).toBeGreaterThan(before);

    getJobs.mockResolvedValue(paginated([job({ status: 'success' })]));
    await vi.advanceTimersByTimeAsync(3000);
    await waitFor(() => expect(screen.queryByText('Refreshing')).toBeNull());

    const settled = getJobs.mock.calls.length;
    await vi.advanceTimersByTimeAsync(3000);
    expect(getJobs.mock.calls.length).toBe(settled);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(getJobs.mock.calls.length).toBeGreaterThan(settled);
  });
});
