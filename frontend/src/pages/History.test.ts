import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { decision, instance, job, paginated, rule } from '../test/fixtures';
import { api, ApiError } from '../api/client';
import { answerConfirmation } from '../test/confirm';
import History from './History.svelte';
import { publishAccess } from '../lib/access.svelte';

const show = () => renderWithI18n(History);

// Nothing a revert of many could take, and nothing to filter, unless a test says otherwise.
beforeEach(() => {
  vi.spyOn(api, 'countDecisions').mockResolvedValue(0);
  vi.spyOn(api, 'getTasks').mockResolvedValue(paginated([]));
  vi.spyOn(api, 'getInstances').mockResolvedValue([]);
  vi.spyOn(api, 'getCategories').mockResolvedValue([]);
  vi.spyOn(api, 'getRules').mockResolvedValue([]);
});

/** A field of the filter panel, opened first as a reader opens it. */
function filterField(label: string): HTMLElement {
  const toggle = screen.getByRole('button', { name: /^Filters/ });
  if (toggle.getAttribute('aria-expanded') !== 'true') toggle.click();
  return screen.getByLabelText(label, { selector: 'select, input' });
}

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

  /** A move the Arr took and is still making is neither done nor revertible yet. */
  it('says a requested move is still being made, and offers no revert for it', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'requested', applied_at: '2026-08-27 11:00:00' })]),
    );
    show();

    expect(await screen.findByText('Requested')).toBeTruthy();
    expect(
      screen.getByText('The Arr is moving the files. The next sync confirms the move.'),
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Revert/ })).toBeNull();
  });

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

    expect(await screen.findByText('Move "Akira" back to /films?')).toBeTruthy();
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

    await waitFor(() => expect(screen.queryByText(/Move "Akira" back/)).toBeNull());
    expect(revertDecisions).not.toHaveBeenCalled();
  });

  it('stops following a revert once the screen closes', async () => {
    const revertDecisions = vi.spyOn(api, 'revertDecisions').mockReturnValue(new Promise(() => {}));
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true })]),
    );
    const { unmount } = show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Put back' }));
    await waitFor(() => expect(revertDecisions).toHaveBeenCalled());
    const signal = nthCall(revertDecisions)[3]?.signal;
    expect(signal?.aborted).toBe(false);

    unmount();
    expect(signal?.aborted).toBe(true);
  });

  it.each([
    ['leaves the files alone unless that box is ticked', false],
    ['moves the files back when that box is ticked', true],
  ])('%s', async (_claim, moveFiles) => {
    const revertDecisions = vi.spyOn(api, 'revertDecisions').mockResolvedValue({
      requested: 1,
      applied: 1,
      failed: 0,
      skipped: 0,
      moving: 0,
      superseded: 0,
      stopped: null,
      errors: [],
    });
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true })]),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    if (moveFiles)
      await userEvent.click(await screen.findByLabelText('Move the files on disk back as well?'));
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

  it('asks the question the backend raises, and reverts once it is answered', async () => {
    const revertDecisions = vi
      .spyOn(api, 'revertDecisions')
      .mockRejectedValueOnce(unreachable())
      .mockResolvedValueOnce({
        requested: 1,
        applied: 1,
        failed: 0,
        skipped: 0,
        moving: 0,
        superseded: 0,
        stopped: null,
        errors: [],
      });
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
    expect(await screen.findByText('Moves reverted: 1')).toBeTruthy();
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

  it('reports a revert that restored nothing as a failure, not a success', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([decision({ status: 'applied', revertible: true, media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'revertDecisions').mockResolvedValue({
      requested: 1,
      applied: 0,
      failed: 1,
      skipped: 0,
      moving: 0,
      superseded: 0,
      stopped: null,
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

  it('takes the previous success off screen when the next revert is refused', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ status: 'applied', revertible: true, media_title: 'Akira' }),
        decision({ status: 'applied', revertible: true, media_title: 'Heat' }),
      ]),
    );
    vi.spyOn(api, 'revertDecisions')
      .mockResolvedValueOnce({
        requested: 1,
        applied: 1,
        failed: 0,
        skipped: 0,
        moving: 0,
        superseded: 0,
        stopped: null,
        errors: [],
      })
      .mockRejectedValueOnce(new ApiError('The Arr refused the move', 502, 'bad_gateway'));
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
    );
    expect(await screen.findByText('Moves reverted: 1')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Heat/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
    );

    expect(await screen.findByText(/The Arr refused the move/)).toBeTruthy();
    expect(screen.queryByText('Moves reverted: 1')).toBeNull();
  });

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
   * A title is asked of the server once the typing stops, not once per letter, and from the first
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

  it('names what caused a decision, and stays silent when nothing recorded it', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ media_title: 'Akira', actor: 'schedule' }),
        decision({ media_title: 'Totoro', actor: null }),
      ]),
    );
    renderWithI18n(History);

    const row = await screen.findByRole('row', { name: /Akira/ });
    expect(within(row).getByTitle('Triggered by')).toHaveTextContent('Schedule');

    const older = screen.getByRole('row', { name: /Totoro/ });
    expect(within(older).queryByTitle('Triggered by')).toBeNull();
  });

  it('names who asked, beside what caused it, and only when a mode named one', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ media_title: 'Akira', actor: 'manual', subject: 'alice' }),
        decision({ media_title: 'Totoro', actor: 'schedule', subject: null }),
      ]),
    );
    renderWithI18n(History);

    const row = await screen.findByRole('row', { name: /Akira/ });
    expect(within(row).getByTitle('Triggered by')).toHaveTextContent('Manual');
    expect(within(row).getByTitle('Performed by')).toHaveTextContent('alice');

    const swept = screen.getByRole('row', { name: /Totoro/ });
    expect(within(swept).getByTitle('Triggered by')).toHaveTextContent('Schedule');
    expect(within(swept).queryByTitle('Performed by')).toBeNull();
  });

  /**
   * The table is read again before a revert is reported, and the revert's success keeps a failed
   * read on screen, or a stale table sits under a green banner.
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
      moving: 0,
      superseded: 0,
      stopped: null,
      errors: [],
    });
    show();

    await fireEvent.click(await screen.findByRole('button', { name: /Revert – Akira/ }));
    await fireEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Put back' }),
    );

    expect(await screen.findByText('Moves reverted: 1')).toBeTruthy();
    expect(screen.getByText('The history could not be read')).toBeTruthy();
  });

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

    await userEvent.selectOptions(filterField('Status'), 'applied');

    await waitFor(() =>
      expect(api.getDecisions).toHaveBeenLastCalledWith(
        expect.objectContaining({ status: 'applied' }),
        expect.anything(),
      ),
    );
    expect(screen.getByText('The Arr refused the move')).toBeTruthy();
  });
});

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
    moving: 0,
    superseded: 0,
    stopped: null,
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

describe('narrowing and reverting many', () => {
  const report = {
    candidates: 3,
    applied: 3,
    failed: 0,
    skipped: 0,
    batches_run: 1,
    batches_planned: 1,
    moving: 0,
    superseded: 0,
    stopped_early: false,
    stopped: null,
    errors: [],
  };

  it('asks the server for the instance, rule, simulation and days chosen', async () => {
    vi.spyOn(api, 'getInstances').mockResolvedValue([instance()]);
    vi.spyOn(api, 'getRules').mockResolvedValue([rule()]);
    vi.spyOn(api, 'getTasks').mockResolvedValue(
      paginated([job({ kind: 'simulate', result: { simulation_id: 's9' } })]),
    );
    const getDecisions = vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([]));
    show();

    filterField('Rule');
    await screen.findByRole('option', { name: 'Japanese animation' });
    await userEvent.selectOptions(filterField('Instance'), 'i1');
    await userEvent.selectOptions(filterField('Rule'), 'r1');
    await userEvent.selectOptions(filterField('Simulation'), 's9');
    await fireEvent.change(filterField('Decided from'), {
      target: { value: '2026-10-17' },
    });
    await fireEvent.change(filterField('Decided until'), {
      target: { value: '2026-10-18' },
    });

    await waitFor(() =>
      expect(getDecisions).toHaveBeenLastCalledWith(
        expect.objectContaining({
          instance_id: 'i1',
          rule_id: 'r1',
          simulation_id: 's9',
          since: new Date(2026, 9, 17).toISOString(),
          until: new Date(2026, 9, 19).toISOString(),
          page: 1,
        }),
        expect.any(AbortSignal),
      ),
    );
    expect(api.countDecisions).toHaveBeenLastCalledWith(
      expect.objectContaining({ instance_id: 'i1', revertible: true }),
      expect.any(AbortSignal),
    );
  });

  it('offers no revert of many while nothing listed can be reverted', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([decision({ status: 'applied' })]));
    show();

    await screen.findByText('Akira');
    expect(screen.queryByRole('button', { name: /Revert all/ })).toBeNull();
  });

  /** The server counts and asks, so the reader confirms the count it will act on. */
  it('reverts every move the filters match once the server question is answered', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([decision({ status: 'applied' })]));
    vi.spyOn(api, 'countDecisions').mockResolvedValue(3);
    vi.spyOn(api, 'getRules').mockResolvedValue([rule()]);
    const revertAll = vi
      .spyOn(api, 'revertAllDecisions')
      .mockRejectedValueOnce(
        new ApiError('Moves to revert: 3.', 409, 'confirmation_required', null, 'batch'),
      )
      .mockResolvedValue(report);
    show();
    expect(await screen.findByRole('button', { name: 'Revert all (3)' })).toBeTruthy();

    filterField('Rule');
    await screen.findByRole('option', { name: 'Japanese animation' });
    await userEvent.selectOptions(filterField('Rule'), 'r1');
    await userEvent.click(
      await screen.findByRole('button', { name: 'Revert the matching moves (3)' }),
    );
    expect(await answerConfirmation()).toContain('Moves to revert: 3.');

    expect(await screen.findByText('Moves reverted: 3 of 3. Batches: 1.')).toBeTruthy();
    expect(nthCall(revertAll)[0]).toEqual({ filter: { rule_id: 'r1' } });
    expect(nthCall(revertAll, 1)[2]).toEqual(['batch']);
  });

  it('says a revert of many stopped by a refused slice left the rest applied', async () => {
    vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([]));
    vi.spyOn(api, 'countDecisions').mockResolvedValue(3);
    vi.spyOn(api, 'revertAllDecisions').mockResolvedValue({
      ...report,
      applied: 1,
      failed: 2,
      batches_planned: 2,
      stopped_early: true,
      stopped: 'refused',
      errors: [{ decision_id: 'd1', media_title: 'Akira', message: 'refused' }],
    });
    show();

    await userEvent.click(await screen.findByRole('button', { name: 'Revert all (3)' }));

    expect(
      await screen.findByText(
        'Stopped after a failure. Reverted: 1 of 3. Batches run: 1 of 2. The rest stays applied.',
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        'Stopped: the Arr refused every move of a batch. The rest was not attempted.',
      ),
    ).toBeTruthy();
    expect(screen.getByText('Akira: refused')).toBeTruthy();
  });
});

describe('for a reader', () => {
  /** A revert moves files back, which a reader is refused, one move or all of them. */
  it('offers the owner the reverts, and shows a reader the moves without them', async () => {
    vi.mocked(api.countDecisions).mockResolvedValue(3);
    vi.spyOn(api, 'getDecisions').mockResolvedValue(
      paginated([
        decision({ status: 'applied', revertible: true, applied_at: '2026-08-27 11:00:00' }),
      ]),
    );
    const offered = () =>
      [/^Revert – Akira$/, /^Revert all/].filter((name) => screen.queryByRole('button', { name }));
    const { unmount } = show();
    await screen.findByRole('button', { name: 'Revert all (3)' });
    expect(offered()).toHaveLength(2);
    unmount();

    publishAccess(true);
    show();

    expect(await screen.findByText('Akira')).toBeInTheDocument();
    await waitFor(() => expect(api.countDecisions).toHaveBeenCalledTimes(2));
    expect(offered()).toEqual([]);
    expect(screen.queryByLabelText(/Move the files on disk too/)).toBeNull();
  });
});
