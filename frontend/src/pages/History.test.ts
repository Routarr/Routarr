import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { decision, paginated } from '../test/fixtures';
import { api, ApiError } from '../api/client';
import { answerConfirmation } from '../test/confirm';
import History from './History.svelte';

/**
 * The audit trail, and the only screen that can undo a move.
 *
 * Reverting asks once, through one dialog. Chaining two ("revert?", then "move
 * the files too?") gives the second no context, and cancelling it would mean
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
  RevertConfirm: 'Put back',
  Cancel: 'Cancel',
  ConfirmRevert: 'Send "{title}" back to {path}?',
  ConfirmRevertFiles: 'Move the files back too',
  RevertResult: '{count} reverted',
  NothingReverted: 'Nothing was reverted.',
  NothingRevertedBecause: 'Nothing was reverted: {reason}',
  FilterByStatus: 'Filter by status',
  SearchATitle: 'Search a title',
  NoDecisionRecorded: 'Nothing decided yet',
  AllStatuses: 'All statuses',
  StatusApplied: 'applied',
  StatusPending: 'pending',
  StatusFailed: 'failed',
  StatusSkipped: 'skipped',
  PageOf: 'Page {page} of {total}',
  DecisionCount: 'Decisions: {count}',
  Next: 'Next',
};

const show = () => renderWithI18n(History, { strings: STRINGS });

afterEach(() => vi.restoreAllMocks());

describe('History', () => {
  it('offers a revert on a move that was applied', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ status: 'applied', revertible: true, applied_at: '2026-08-27 11:00:00' }),
      ]),
    );
    show();

    expect(await screen.findByRole('button', { name: /Revert – Akira/ })).toBeTruthy();
  });

  /**
   * Whether a move can still be undone is the server's verdict: one never
   * applied, one already undone, and one another move has followed since all
   * arrive with `revertible` false. Offering the button anyway sends the
   * executor a request it can only refuse.
   */
  it('offers no revert where the server says the move cannot be undone', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', applied_at: '2026-08-27 11:00:00' })]),
    );
    show();

    await screen.findByText('Akira');
    expect(screen.queryByRole('button', { name: /Revert – Akira/ })).toBeNull();
  });

  it('asks once, naming the film and where it would go back to', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true, current_root_folder: '/films' })]),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));

    // The path in an isolate, so a right-to-left sentence keeps its slash first.
    expect(await screen.findByText('Send "Akira" back to \u2068/films\u2069?')).toBeTruthy();
  });

  /** Cancel means stop, not "revert quietly". */
  it('reverts nothing when the dialog is cancelled', async () => {
    const revertDecisions = vi.spyOn(api, 'revertDecisions');
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true })]),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(screen.queryByText(/Send "Akira" back/)).toBeNull());
    expect(revertDecisions).not.toHaveBeenCalled();
  });

  it.each([
    ['leaves the files alone unless that box is ticked', false],
    ['moves the files back when that box is ticked', true],
  ])('%s', async (_claim, moveFiles) => {
    const revertDecisions = vi
      .spyOn(api, 'revertDecisions')
      .mockResolvedValue({ requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] });
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true })]),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    if (moveFiles) await userEvent.click(await screen.findByLabelText('Move the files back too'));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Put back' }));

    await waitFor(() => expect(revertDecisions).toHaveBeenCalledTimes(1));
    expect(nthCall(revertDecisions)[1]).toBe(moveFiles);
  });

  const unreachable = () =>
    new ApiError(
      '/movies/standard is not answering.',
      409,
      'confirmation_required',
      null,
      'unreachable',
    );

  /**
   * A revert writes into the folder the move came from, and the backend asks
   * there what it asks before an apply. The question lifts only its own name,
   * so the second send carries that name and nothing else.
   */
  it('asks the question the backend raises, and reverts once it is answered', async () => {
    const revertDecisions = vi
      .spyOn(api, 'revertDecisions')
      .mockRejectedValueOnce(unreachable())
      .mockResolvedValueOnce({ requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] });
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true })]),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Put back' }));

    expect(await answerConfirmation()).toBe('/movies/standard is not answering.');
    await waitFor(() => expect(revertDecisions).toHaveBeenCalledTimes(2));
    expect(nthCall(revertDecisions, 1)[2]).toEqual(['unreachable']);
    expect(await screen.findByText('1 reverted')).toBeTruthy();
  });

  it('reverts nothing, and reports no failure, when the question is declined', async () => {
    const revertDecisions = vi.spyOn(api, 'revertDecisions').mockRejectedValueOnce(unreachable());
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true })]),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Put back' }));
    await answerConfirmation(null);

    expect(revertDecisions).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  /**
   * A revert that restored nothing did not do what was asked, and in the success
   * banner the reason reads as good news under a green tick.
   */
  it('reports a revert that restored nothing as a failure, not a success', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true, media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'revertDecisions').mockResolvedValue({
      requested: 1,
      applied: 0,
      failed: 1,
      skipped: 0,
      errors: [{ decision_id: 'd1', message: 'The file is no longer there' }],
    } as never);
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
    );

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Nothing was reverted: The file is no longer there',
    );
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
  });

  /**
   * The banner stays until something replaces it. Left standing, the first
   * revert's count reads as the outcome of the second, printed above the
   * reason that one was refused.
   */
  it('takes the previous success off screen when the next revert is refused', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ status: 'applied', revertible: true, media_title: 'Akira' }),
        decision({ status: 'applied', revertible: true, media_title: 'Heat' }),
      ]),
    );
    vi.spyOn(api, 'revertDecisions')
      .mockResolvedValueOnce({ requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] })
      .mockRejectedValueOnce(new ApiError('The Arr refused the move', 502, 'bad_gateway'));
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
    );
    expect(await screen.findByText('1 reverted')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Heat/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
    );

    expect(await screen.findByText(/The Arr refused the move/)).toBeTruthy();
    expect(screen.queryByText('1 reverted')).toBeNull();
  });

  /** The history is paged by the server, and the pager asks it for the next page. */
  it('counts every decision under the table, and asks the server for the next page', async () => {
    const getDecisions = vi
      .spyOn(api, 'getDecisions')
      .mockResolvedValue(paginated([decision()], { total_pages: 3, total: 150 }));
    show();

    expect(await screen.findByText('Page 1 of 3 · Decisions: 150')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));

    await waitFor(() =>
      expect(getDecisions).toHaveBeenLastCalledWith(
        expect.objectContaining({ page: 2 }),
        expect.any(AbortSignal),
      ),
    );
  });

  /**
   * Every search is a query against the operator's own server, so a title is
   * asked for once the typing stops, not once per letter, and from the first
   * page: the page the reader was on may not exist in the narrower list.
   */
  it('asks for a typed title once the typing stops, from the first page', async () => {
    const getDecisions = vi
      .spyOn(api, 'getDecisions')
      .mockResolvedValue(paginated([decision()], { total_pages: 3, total: 150 }));
    show();
    await screen.findByText('Akira');
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() => expect(getDecisions).toHaveBeenCalledTimes(2));

    await userEvent.type(screen.getByRole('searchbox', { name: 'Search a title' }), 'heat');

    await waitFor(() => expect(getDecisions).toHaveBeenCalledTimes(3));
    expect(nthCall(getDecisions, 2)[0]).toMatchObject({ search: 'heat', page: 1 });
    expect(getDecisions.mock.calls.map(([filters]) => filters?.search)).toEqual([
      undefined,
      undefined,
      'heat',
    ]);
  });

  /**
   * The row says whether the nightly sweep proposed a move or a person did,
   * and says nothing, rather than guessing, when the row recorded no cause.
   */
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

  /**
   * "Manually" is not an answer once several people can sign in, and a key can
   * be a script rather than any of them. The name sits beside the trigger,
   * since the scheduler has one and no name.
   */
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
      .mockResolvedValueOnce(
        paginated([decision({ status: 'applied', revertible: true, media_title: 'Akira' })]),
      )
      .mockRejectedValue(new ApiError('The history could not be read', 409, 'conflict'));
    vi.spyOn(api, 'revertDecisions').mockResolvedValue({
      requested: 1,
      applied: 1,
      failed: 0,
      skipped: 0,
      errors: [],
    });
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
    );

    expect(await screen.findByText('1 reverted')).toBeTruthy();
    expect(screen.getByText('The history could not be read')).toBeTruthy();
  });

  /** A refusal stays until it is dismissed or replaced, whatever reloads the table. */
  it('keeps a refused revert on screen when the table reloads', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true, media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'revertDecisions').mockRejectedValue(
      new ApiError('The Arr refused the move', 409, 'conflict'),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
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

/** A reverted row stays and loses its Revert: the next row's Revert takes the focus. */
it('hands the focus to the next Revert once a row cannot be reverted again', async () => {
  const first = decision({ status: 'applied', revertible: true });
  const next = decision({
    id: 'd2',
    media_title: 'Perfect Blue',
    status: 'applied',
    revertible: true,
  });
  vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([first, next]));
  vi.spyOn(api, 'revertDecisions').mockResolvedValue({
    requested: 1,
    applied: 1,
    failed: 0,
    skipped: 0,
    errors: [],
  });
  show();

  await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
  const dialog = await screen.findByRole('dialog');
  vi.spyOn(api, 'getDecisions').mockResolvedValue(
    paginated([{ ...first, revertible: false }, next]),
  );
  await fireEvent.click(within(dialog).getByRole('button', { name: 'Put back' }));

  await waitFor(() =>
    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: /Revert – Perfect Blue/ }),
    ),
  );
});
