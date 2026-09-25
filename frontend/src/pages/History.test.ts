import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { decision, paginated } from '../test/fixtures';
import { api, ApiError } from '../api/client';
import History from './History.svelte';

/**
 * The audit trail, and the only screen that can undo a move.
 *
 * Reverting asks once, through one dialog. Chaining two — "revert?", then "move
 * the files too?" — gives the second no context, and cancelling it would mean
 * "revert without moving files" rather than "stop": a Cancel button that does
 * not cancel.
 */

const STRINGS = {
  AuditHistory: 'History',
  TriggeredBy: 'Triggered by',
  PerformedBy: 'Performed by',
  TriggerSchedule: 'schedule',
  TriggerManual: 'manual',
  Revert: 'Revert',
  Cancel: 'Cancel',
  ConfirmRevert: 'Send “{title}” back to {path}?',
  ConfirmRevertFiles: 'Move the files back too',
  RevertResult: '{count} reverted',
  RevertNothing: 'Nothing was reverted',
  FilterByStatus: 'Filter by status',
  SearchATitle: 'Search a title',
  NoDecisionRecorded: 'Nothing decided yet',
  AllStatuses: 'All statuses',
  StatusApplied: 'applied',
  StatusPending: 'pending',
  StatusFailed: 'failed',
  StatusSkipped: 'skipped',
};

const show = () => renderWithI18n(History, { strings: STRINGS });

afterEach(() => vi.restoreAllMocks());

describe('History', () => {
  it('offers a revert on a move that was applied', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', applied_at: '2026-08-27 11:00:00' })]),
    );
    show();

    expect(await screen.findByRole('button', { name: /Revert — Akira/ })).toBeTruthy();
  });

  /**
   * A move that never happened has nothing to undo, and one already undone has
   * nothing left to undo. Offering the button anyway sends the executor a
   * request it can only refuse.
   */
  it('offers no revert on a decision that was never applied', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([decision({ status: 'pending' })]));
    show();

    await screen.findByText('Akira');
    expect(screen.queryByRole('button', { name: /Revert — Akira/ })).toBeNull();
  });

  it('offers no revert on a decision already reverted', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({
          status: 'applied',
          applied_at: '2026-08-27 11:00:00',
          reverted_at: '2026-08-27 12:00:00',
        }),
      ]),
    );
    show();

    await screen.findByText('Akira');
    expect(screen.queryByRole('button', { name: /Revert — Akira/ })).toBeNull();
  });

  it('asks once, naming the film and where it would go back to', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', current_root_folder: '/films' })]),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Akira/ }));

    expect(await screen.findByText('Send “Akira” back to /films?')).toBeTruthy();
  });

  /** Cancel means stop, not "revert quietly". */
  it('reverts nothing when the dialog is cancelled', async () => {
    const revertDecisions = vi.spyOn(api, 'revertDecisions');
    vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([decision({ status: 'applied' })]));
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Akira/ }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(screen.queryByText(/Send “Akira” back/)).toBeNull());
    expect(revertDecisions).not.toHaveBeenCalled();
  });

  it.each([
    ['leaves the files alone unless that box is ticked', false],
    ['moves the files back when that box is ticked', true],
  ])('%s', async (_claim, moveFiles) => {
    const revertDecisions = vi
      .spyOn(api, 'revertDecisions')
      .mockResolvedValue({ requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] });
    vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([decision({ status: 'applied' })]));
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Akira/ }));
    if (moveFiles) await userEvent.click(await screen.findByLabelText('Move the files back too'));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Revert' }));

    await waitFor(() => expect(revertDecisions).toHaveBeenCalledTimes(1));
    expect(nthCall(revertDecisions)[1]).toBe(moveFiles);
  });

  /**
   * A revert that restored nothing did not do what was asked, and in the success
   * banner the reason reads as good news under a green tick.
   */
  it('reports a revert that restored nothing as a failure, not a success', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'revertDecisions').mockResolvedValue({
      requested: 1,
      applied: 0,
      failed: 1,
      skipped: 0,
      errors: [{ decision_id: 'd1', message: 'The file is no longer there' }],
    } as never);
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revert' }),
    );

    expect(await screen.findByRole('alert')).toHaveTextContent('The file is no longer there');
    expect(screen.queryByRole('status')).toBeNull();
  });

  /**
   * The banner stays until something replaces it. Left standing, the first
   * revert's count reads as the outcome of the second, printed above the
   * reason that one was refused.
   */
  it('takes the previous success off screen when the next revert is refused', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ status: 'applied', media_title: 'Akira' }),
        decision({ status: 'applied', media_title: 'Heat' }),
      ]),
    );
    vi.spyOn(api, 'revertDecisions')
      .mockResolvedValueOnce({ requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] })
      .mockRejectedValueOnce(new ApiError('The Arr refused the move', 502, 'bad_gateway'));
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revert' }),
    );
    expect(await screen.findByText('1 reverted')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Heat/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revert' }),
    );

    expect(await screen.findByText(/The Arr refused the move/)).toBeTruthy();
    expect(screen.queryByText('1 reverted')).toBeNull();
  });

  /**
   * The list is paginated server-side, so a filter that only hid rows in the
   * browser would filter one page of fifty and call it the answer.
   */
  it('asks the server for the filtered set rather than hiding rows on screen', async () => {
    const getDecisions = vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([decision()]));
    show();
    await screen.findByText('Akira');

    await userEvent.selectOptions(screen.getByLabelText('Filter by status'), 'failed');

    await waitFor(() =>
      expect(getDecisions).toHaveBeenCalledWith(
        expect.objectContaining({ status: 'failed' }),
        expect.any(AbortSignal),
      ),
    );
  });

  /// The screen could not say whether the nightly sweep proposed a move or a
  /// person did; the row now carries it, and says nothing when the row predates
  /// the column rather than guessing.
  it('names what caused a decision, and stays silent when nothing recorded it', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ media_title: 'Akira', actor: 'schedule' }),
        decision({ media_title: 'Totoro', actor: null }),
      ]),
    );
    renderWithI18n(History, { strings: STRINGS });

    const row = await screen.findByRole('row', { name: /Akira/ });
    expect(within(row).getByTitle('Triggered by')).toHaveTextContent('schedule');

    const older = screen.getByRole('row', { name: /Totoro/ });
    expect(within(older).queryByTitle('Triggered by')).toBeNull();
  });

  /// "Manually" is not an answer once several people can sign in, and a key can
  /// be a script rather than any of them. The name sits beside the trigger,
  /// since the scheduler has one and no name.
  it('names who asked, beside what caused it, and only when a mode named one', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ media_title: 'Akira', actor: 'manual', subject: 'alice' }),
        decision({ media_title: 'Totoro', actor: 'schedule', subject: null }),
      ]),
    );
    renderWithI18n(History, { strings: STRINGS });

    const row = await screen.findByRole('row', { name: /Akira/ });
    expect(within(row).getByTitle('Triggered by')).toHaveTextContent('manual');
    expect(within(row).getByTitle('Performed by')).toHaveTextContent('alice');

    const swept = screen.getByRole('row', { name: /Totoro/ });
    expect(within(swept).getByTitle('Triggered by')).toHaveTextContent('schedule');
    expect(within(swept).queryByTitle('Performed by')).toBeNull();
  });

  /**
   * The table is read again before a revert is reported. A read that fails
   * leaves the rows as they were, and the revert's success must not take that
   * failure off screen, or a stale table sits under a green banner.
   */
  it('keeps a failed reload on screen beside the revert it followed', async () => {
    vi.spyOn(api, 'getDecisions')
      .mockResolvedValueOnce(paginated([decision({ status: 'applied', media_title: 'Akira' })]))
      .mockRejectedValue(new ApiError('The history could not be read', 409, 'conflict'));
    vi.spyOn(api, 'revertDecisions').mockResolvedValue({
      requested: 1,
      applied: 1,
      failed: 0,
      skipped: 0,
      errors: [],
    });
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revert' }),
    );

    expect(await screen.findByText('1 reverted')).toBeTruthy();
    expect(screen.getByText('The history could not be read')).toBeTruthy();
  });

  /** A refusal stays until it is dismissed or replaced, whatever reloads the table. */
  it('keeps a refused revert on screen when the table reloads', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'revertDecisions').mockRejectedValue(
      new ApiError('The Arr refused the move', 409, 'conflict'),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert — Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revert' }),
    );
    expect(await screen.findByText('The Arr refused the move')).toBeTruthy();

    await userEvent.selectOptions(screen.getByLabelText('Filter by status'), 'applied');

    await waitFor(() =>
      expect(api.getDecisions).toHaveBeenLastCalledWith(
        expect.objectContaining({ status: 'applied' }),
        expect.anything(),
      ),
    );
    expect(screen.getByText('The Arr refused the move')).toBeTruthy();
  });
});
