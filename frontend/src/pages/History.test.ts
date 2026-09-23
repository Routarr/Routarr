import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { decision, paginated } from '../test/fixtures';
import { api } from '../api/client';
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
});
