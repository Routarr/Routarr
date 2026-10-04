import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { paginated } from '../test/fixtures';
import { captureDownloads } from '../test/downloads';
import { api } from '../api/client';
import type { LogEntry } from '../api/types';
import Logs from './Logs.svelte';

/**
 * Every write Routarr has made to an Arr. The export is the part with a trap in
 * it: the endpoint wants the API key in a header, which a plain `<a href>`
 * cannot carry, so it goes through `fetch` and a blob.
 */

const STRINGS = {
  LogsTitle: 'Activity',
  ExportCsv: 'Export CSV',
  NoWritesYet: 'Nothing has been written yet',
  FilterByOutcome: 'Filter by outcome',
  SearchTitleOrDetails: 'Search title or details',
  AllOutcomes: 'All outcomes',
  OutcomeSuccess: 'Succeeded',
  OutcomeFailure: 'Failed',
  StatusSuccess: 'succeeded',
  StatusFailed: 'failed',
  None: '-',
  ExportFailed: 'Export failed with status {status}',
  ActionMove: 'Move',
  ActionRevert: 'Putting back',
  Revert: 'Put it back',
  TriggerManual: 'manual',
  TriggerSchedule: 'schedule',
  PageOf: 'Page {page} of {total}',
  LogEntryCount: 'Entries: {count}',
  Next: 'Next',
};

function entry(over: Partial<LogEntry> = {}): LogEntry {
  return {
    id: 'l1',
    decision_id: 'd1',
    action: 'move',
    details: '/films → /anime',
    success: true,
    error_message: null,
    instance_id: 'i1',
    media_id: 'm1',
    media_title: 'Akira',
    actor: 'schedule',
    subject: null,
    executed_at: '2026-08-27 10:00:00',
    ...over,
  };
}

const show = () => renderWithI18n(Logs, { strings: STRINGS });

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Activity log', () => {
  it('says nothing has been written rather than showing an empty table', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([]));
    show();

    expect(await screen.findByText('Nothing has been written yet')).toBeTruthy();
  });

  /** "Did the nightly sweep do this, or did somebody", and which somebody. */
  it('names what set each write off, and who asked when a mode said', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(
      paginated([
        entry({ id: 'l1', media_title: 'Akira', actor: 'manual', subject: 'alice' }),
        entry({ id: 'l2', media_title: 'Totoro', actor: 'schedule', subject: null }),
      ]),
    );
    show();

    const manual = (await screen.findByText('Akira')).closest('tr') as HTMLElement;
    expect(within(manual).getByText('manual')).toBeTruthy();
    expect(within(manual).getByText('alice')).toBeTruthy();
    const scheduled = screen.getByText('Totoro').closest('tr') as HTMLElement;
    expect(within(scheduled).getByText('schedule')).toBeTruthy();
  });

  /**
   * A failure carries the message the Arr gave back. Showing the generic
   * "failed" badge and dropping the reason leaves the operator with a log that
   * records that something went wrong and nothing about what.
   */
  it('shows the reason a write failed, not only that it failed', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(
      paginated([entry({ success: false, error_message: 'Radarr said 409' })]),
    );
    show();

    expect(await screen.findByText('Radarr said 409')).toBeTruthy();
    expect(screen.getByText('failed')).toBeTruthy();
  });

  /** The badge names what was written, as a noun beside Move, not the button's order. */
  it('names a revert by its noun, not by the button that does it', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry({ action: 'revert' })]));
    show();

    expect(await screen.findByText('Putting back')).toBeTruthy();
    expect(screen.queryByText('Put it back')).toBeNull();
  });

  it('has nothing to export while the list is empty', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([]));
    show();

    const button = await screen.findByRole('button', { name: /export csv/i });
    expect((button as HTMLButtonElement).disabled).toBe(true);
  });

  /**
   * The export is a `fetch` rather than a link precisely so the key travels. As
   * an `<a href>` the download would 401, and the only symptom would be an
   * empty file.
   */
  it('carries the API key when exporting, which a plain link could not', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry()]));
    localStorage.setItem('routarr.apiKey', 'the-key');
    const fetcher = vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(['a,b']) });
    vi.stubGlobal('fetch', fetcher);
    const saved = captureDownloads();

    show();
    await fireEvent.click(await screen.findByRole('button', { name: /export csv/i }));

    await waitFor(() => expect(saved).toHaveLength(1));
    expect((nthCall(fetcher)[1] as { headers: Record<string, string> }).headers['X-Api-Key']).toBe(
      'the-key',
    );
  });

  /** The file holds what the screen shows: the search and the outcome go with it. */
  it('exports what the filters show, under the name of the log', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry()]));
    const exportLogs = vi.spyOn(api, 'exportLogs').mockResolvedValue(new Blob(['a,b']));
    const saved = captureDownloads();
    show();

    await userEvent.type(await screen.findByLabelText('Search title or details'), 'akira');
    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), 'success');
    // The screen speaks in outcomes, and the API takes a boolean.
    await waitFor(() =>
      expect(api.getLogs).toHaveBeenLastCalledWith(
        expect.objectContaining({ search: 'akira', success: true }),
        expect.anything(),
      ),
    );
    await userEvent.click(screen.getByRole('button', { name: /export csv/i }));

    await waitFor(() => expect(saved).toHaveLength(1));
    expect(nthCall(exportLogs)[0]).toMatchObject({ search: 'akira', success: true });
    expect(saved[0]?.name).toBe('routarr-logs.csv');
  });

  /** The log is paged by the server, and the pager asks it for the next page. */
  it('counts every entry under the table, and asks the server for the next page', async () => {
    const getLogs = vi
      .spyOn(api, 'getLogs')
      .mockResolvedValue(paginated([entry()], { total_pages: 3, total: 150 }));
    show();

    expect(await screen.findByText('Page 1 of 3 · Entries: 150')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));

    await waitFor(() =>
      expect(getLogs).toHaveBeenLastCalledWith(
        expect.objectContaining({ page: 2 }),
        expect.any(AbortSignal),
      ),
    );
  });

  /**
   * Every search is a query against the operator's own server, so a term is
   * asked for once the typing stops, not once per letter, and from the first
   * page: the page the reader was on may not exist in the narrower list.
   */
  it('asks for a typed term once the typing stops, from the first page', async () => {
    const getLogs = vi
      .spyOn(api, 'getLogs')
      .mockResolvedValue(paginated([entry()], { total_pages: 3, total: 150 }));
    show();
    await screen.findByText('Akira');
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() => expect(getLogs).toHaveBeenCalledTimes(2));

    await userEvent.type(
      screen.getByRole('searchbox', { name: 'Search title or details' }),
      'heat',
    );

    await waitFor(() => expect(getLogs).toHaveBeenCalledTimes(3));
    expect(nthCall(getLogs, 2)[0]).toMatchObject({ search: 'heat', page: 1 });
    expect(getLogs.mock.calls.map(([filters]) => filters?.search)).toEqual([
      undefined,
      undefined,
      'heat',
    ]);
  });

  it('asks for everything when the filter is cleared, rather than for failures', async () => {
    const getLogs = vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry()]));
    show();
    await screen.findByText('Akira');

    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), 'success');
    await waitFor(() => expect(getLogs).toHaveBeenCalledTimes(2));
    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), '');

    // `undefined`, not `false`: an empty filter means no filter, and sending
    // `success=false` would quietly show only the failures.
    await waitFor(() => expect(getLogs).toHaveBeenCalledTimes(3));
    expect(nthCall(getLogs, 2)[0]).toMatchObject({ success: undefined });
  });

  /** A failed export is not a load to retry: the list is fine, the file is not. */
  it('reports a failed export without offering to reload the list', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry()]));
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ ok: false, status: 503, text: async () => '' }),
    );

    show();
    await fireEvent.click(await screen.findByRole('button', { name: /export csv/i }));

    expect(await screen.findByText('Export failed with status 503')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  it('takes a failed export off screen once the next one downloads', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry()]));
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce({ ok: false, status: 503, text: async () => '' })
        .mockResolvedValueOnce({ ok: true, blob: async () => new Blob(['a,b']) }),
    );
    const saved = captureDownloads();

    show();
    const exportCsv = await screen.findByRole('button', { name: /export csv/i });
    await fireEvent.click(exportCsv);
    expect(await screen.findByText('Export failed with status 503')).toBeTruthy();

    await fireEvent.click(exportCsv);

    await waitFor(() => expect(saved).toHaveLength(1));
    await waitFor(() => expect(screen.queryByText('Export failed with status 503')).toBeNull());
    expect(screen.queryByRole('alert')).toBeNull();
  });
});
