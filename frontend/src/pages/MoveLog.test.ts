import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { paginated } from '../test/fixtures';
import { captureDownloads } from '../test/downloads';
import { api, ApiError } from '../api/client';
import type { LogEntry } from '../api/types';
import MoveLog from './MoveLog.svelte';

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
    task_id: null,
    executed_at: '2026-08-27 10:00:00',
    ...over,
  };
}

const show = () => renderWithI18n(MoveLog);

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Move log', () => {
  it('says nothing has been written rather than showing an empty table', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([]));
    show();

    expect(
      await screen.findByText('Nothing has been written to Radarr or Sonarr yet.'),
    ).toBeTruthy();
  });

  it('names what set each write off, and who asked when a mode said', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(
      paginated([
        entry({ id: 'l1', media_title: 'Akira', actor: 'manual', subject: 'alice' }),
        entry({ id: 'l2', media_title: 'Totoro', actor: 'schedule', subject: null }),
      ]),
    );
    show();

    const manual = (await screen.findByText('Akira')).closest('tr') as HTMLElement;
    expect(within(manual).getByText('Manual')).toBeTruthy();
    expect(within(manual).getByText('alice').closest('td')).toHaveTextContent(
      /Triggered by\s*Manual.*Performed by\s*alice/,
    );
    const scheduled = screen.getByText('Totoro').closest('tr') as HTMLElement;
    expect(within(scheduled).getByText('Schedule')).toBeTruthy();
  });

  /** Beside the outcome, which a phone keeps in view with the title. */
  it('shows the reason a write failed under its outcome, after the title', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(
      paginated([entry({ success: false, error_message: 'Radarr said 409' })]),
    );
    show();

    const reason = await screen.findByText('Radarr said 409');
    expect(reason.closest('td')).toBe(screen.getByText('Failed').closest('td'));
    expect(screen.getAllByRole('columnheader').map((header) => header.textContent)).toEqual([
      'Media',
      'Outcome',
      'When',
      'Action',
      'Details',
    ]);
  });

  /**
   * The badge names what was written, as a noun beside Move, not the button's
   * order. English writes the two alike, and French does not.
   */
  it('names a revert by its noun, not by the button that does it', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([entry({ action: 'revert' })]));
    renderWithI18n(MoveLog, {
      language: 'fr',
      strings: { ActionRevert: 'Annulation', Revert: 'Annuler le déplacement' },
    });

    expect(await screen.findByText('Annulation')).toBeTruthy();
    expect(screen.queryByText('Annuler le déplacement')).toBeNull();
  });

  it('has nothing to export while the list is empty', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([]));
    show();

    const button = await screen.findByRole('button', { name: /export csv/i });
    expect((button as HTMLButtonElement).disabled).toBe(true);
  });

  it('says a refused export rather than saving the refusal as the file', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([entry()]));
    vi.spyOn(api, 'exportMoveLog').mockRejectedValue(new ApiError('', 403, 'forbidden'));
    const saved = captureDownloads();

    show();
    await fireEvent.click(await screen.findByRole('button', { name: /export csv/i }));

    expect(await screen.findByText(/403/)).toBeTruthy();
    expect(saved).toHaveLength(0);
  });

  it('exports what the filters show, under the name of the log', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([entry()]));
    const exportMoveLog = vi
      .spyOn(api, 'exportMoveLog')
      .mockResolvedValue({ blob: new Blob(['a,b']), truncated: false });
    const saved = captureDownloads();
    show();

    await userEvent.type(await screen.findByLabelText('Search title or details'), 'akira');
    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), 'success');
    // The screen speaks in outcomes, and the API takes a boolean.
    await waitFor(() =>
      expect(api.getMoveLog).toHaveBeenLastCalledWith(
        expect.objectContaining({ search: 'akira', success: true }),
        expect.anything(),
      ),
    );
    await userEvent.click(screen.getByRole('button', { name: /export csv/i }));

    await waitFor(() => expect(saved).toHaveLength(1));
    expect(nthCall(exportMoveLog)[0]).toMatchObject({ search: 'akira', success: true });
    expect(saved[0]?.name).toBe('routarr-logs.csv');
  });

  /** A file cut at the server's ceiling would otherwise read as the whole log. */
  it('warns that an export cut short holds the newest entries only', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([entry()]));
    vi.spyOn(api, 'exportMoveLog').mockResolvedValue({ blob: new Blob(['a,b']), truncated: true });
    const saved = captureDownloads();
    show();

    await userEvent.click(await screen.findByRole('button', { name: /export csv/i }));

    await waitFor(() => expect(saved).toHaveLength(1));
    expect(
      await screen.findByText(
        'The file holds the newest 50,000 entries: narrow the filters to export the rest',
      ),
    ).toBeTruthy();
  });

  it('counts every entry under the table, and asks the server for the next page', async () => {
    const getMoveLog = vi
      .spyOn(api, 'getMoveLog')
      .mockResolvedValue(paginated([entry()], { total_pages: 3, total: 150 }));
    show();

    expect(await screen.findByText('Page 1 of 3 · Entries: 150')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));

    await waitFor(() =>
      expect(getMoveLog).toHaveBeenLastCalledWith(
        expect.objectContaining({ page: 2 }),
        expect.any(AbortSignal),
      ),
    );
  });

  /**
   * Each letter would be a query to the operator's own server, and the page the reader was
   * on may not exist in the narrower list.
   */
  it('asks for a typed term once the typing stops, from the first page', async () => {
    const getMoveLog = vi
      .spyOn(api, 'getMoveLog')
      .mockResolvedValue(paginated([entry()], { total_pages: 3, total: 150 }));
    show();
    await screen.findByText('Akira');
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() => expect(getMoveLog).toHaveBeenCalledTimes(2));

    await userEvent.type(
      screen.getByRole('searchbox', { name: 'Search title or details' }),
      'heat',
    );

    await waitFor(() => expect(getMoveLog).toHaveBeenCalledTimes(3));
    expect(nthCall(getMoveLog, 2)[0]).toMatchObject({ search: 'heat', page: 1 });
    expect(getMoveLog.mock.calls.map(([filters]) => filters?.search)).toEqual([
      undefined,
      undefined,
      'heat',
    ]);
  });

  it('asks for everything when the filter is cleared, rather than for failures', async () => {
    const getMoveLog = vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([entry()]));
    show();
    await screen.findByText('Akira');

    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), 'success');
    await waitFor(() => expect(getMoveLog).toHaveBeenCalledTimes(2));
    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), '');

    // `undefined`, not `false`: `success=false` would quietly show only the failures.
    await waitFor(() => expect(getMoveLog).toHaveBeenCalledTimes(3));
    expect(nthCall(getMoveLog, 2)[0]).toMatchObject({ success: undefined });
  });

  it('reports a failed export without offering to reload the list', async () => {
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([entry()]));
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
    vi.spyOn(api, 'getMoveLog').mockResolvedValue(paginated([entry()]));
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce({ ok: false, status: 503, text: async () => '' })
        .mockResolvedValueOnce({
          ok: true,
          headers: new Headers(),
          blob: async () => new Blob(['a,b']),
        }),
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
