import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { paginated } from '../test/fixtures';
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
  None: '—',
  ExportFailed: 'Export failed with status {status}',
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
    executed_at: '2026-08-27 10:00:00',
    ...over,
  };
}

const show = () => renderWithI18n(Logs, { strings: STRINGS });

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe('Activity log', () => {
  it('says nothing has been written rather than showing an empty table', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([]));
    show();

    expect(await screen.findByText('Nothing has been written yet')).toBeTruthy();
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

  it('has nothing to export while the list is empty', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([]));
    show();

    const button = await screen.findByRole('button', { name: /export csv/i });
    expect((button as HTMLButtonElement).disabled).toBe(true);
  });

  /**
   * The export is a `fetch` rather than a link precisely so the key travels. If
   * it ever went back to an `<a href>` the download would 401 and the only
   * symptom would be an empty file.
   */
  it('carries the API key when exporting, which a plain link could not', async () => {
    vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry()]));
    localStorage.setItem('routarr.apiKey', 'the-key');
    const fetcher = vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(['a,b']) });
    vi.stubGlobal('fetch', fetcher);
    // Patched onto the real `URL`, not over it: replacing the global would take
    // the constructor with it, and the API client builds every request URL.
    Object.assign(URL, { createObjectURL: () => 'blob:x', revokeObjectURL: () => {} });

    show();
    await fireEvent.click(await screen.findByRole('button', { name: /export csv/i }));

    await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(1));
    expect((nthCall(fetcher)[1] as { headers: Record<string, string> }).headers['X-Api-Key']).toBe(
      'the-key',
    );
  });

  it('asks the server for the outcome the user picked', async () => {
    const getLogs = vi.spyOn(api, 'getLogs').mockResolvedValue(paginated([entry()]));
    show();
    await screen.findByText('Akira');

    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), 'success');

    // The screen speaks in outcomes; the API takes a boolean.
    await waitFor(() =>
      expect(getLogs).toHaveBeenCalledWith(
        expect.objectContaining({ success: true }),
        expect.any(AbortSignal),
      ),
    );
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
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 503 }));

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
        .mockResolvedValueOnce({ ok: false, status: 503 })
        .mockResolvedValueOnce({ ok: true, blob: async () => new Blob(['a,b']) }),
    );
    const createObjectURL = vi.fn(() => 'blob:x');
    Object.assign(URL, { createObjectURL, revokeObjectURL: () => {} });

    show();
    const exportCsv = await screen.findByRole('button', { name: /export csv/i });
    await fireEvent.click(exportCsv);
    expect(await screen.findByText('Export failed with status 503')).toBeTruthy();

    await fireEvent.click(exportCsv);

    await waitFor(() => expect(createObjectURL).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByText('Export failed with status 503')).toBeNull());
    expect(screen.queryByRole('alert')).toBeNull();
  });
});
