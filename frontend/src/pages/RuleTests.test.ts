import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { paginated } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import type { RuleTest, RuleTestRun } from '../api/types';
import RuleTests from './RuleTests.svelte';
import { publishAccess } from '../lib/access.svelte';
import { answerConfirmation } from '../test/confirm';

const CASE: RuleTest = {
  id: 't1',
  name: 'Akira stays anime',
  media_type: 'movie',
  expected_category: 'anime',
  source_media_title: 'Akira',
  evaluated_at: '2026-09-01 10:00:00',
  created_at: '2026-09-01 10:00:00',
  last_passed: null,
  last_checked_at: null,
  subject: null,
};

const PASSED: RuleTestRun = {
  total: 1,
  passed: 1,
  failed: 0,
  results: [
    {
      id: 't1',
      name: CASE.name,
      expected_category: 'anime',
      actual_category: 'anime',
      passed: true,
      matched_rule: 'Anime',
      error: null,
      error_key: null,
    },
  ],
};

const OTHER_CASE: RuleTest = { ...CASE, id: 't3', name: 'Paprika stays anime' };

afterEach(() => vi.restoreAllMocks());

describe('RuleTests', () => {
  /** A case run after a sync or at its pin shows that verdict before anyone presses Run. */
  it('shows the verdict the last run kept, and when it ran', async () => {
    const failing = { ...CASE, last_passed: false, last_checked_at: '2026-10-18 09:00:00' };
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([failing, OTHER_CASE]));
    renderWithI18n(RuleTests);

    const row = await screen.findByRole('row', { name: /Akira stays anime/ });
    expect(within(row).getByText('Fails')).toBeTruthy();
    expect(within(row).getByText(/^Checked /)).toBeTruthy();
    const other = screen.getByRole('row', { name: /Paprika stays anime/ });
    expect(within(other).getByText('not run yet')).toBeTruthy();
  });

  /** A run takes the whole library: the button says it is running, not only spins. */
  it('says the tests are running while they run', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE]));
    vi.spyOn(api, 'runRuleTests').mockReturnValue(new Promise(() => {}));
    renderWithI18n(RuleTests);

    await fireEvent.click(await screen.findByRole('button', { name: 'Run tests' }));

    const running = await screen.findByRole('button', { name: 'Running tests…' });
    expect((running as HTMLButtonElement).disabled).toBe(true);
  });

  /**
   * A run that fails leaves nothing of the run before it. Kept, the earlier
   * summary and verdicts read as the answer to the run that was just refused.
   */
  it('takes the previous run off screen when the next one fails', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE]));
    vi.spyOn(api, 'runRuleTests')
      .mockResolvedValueOnce(PASSED)
      .mockRejectedValueOnce(new ApiError('The rules could not be read', 409, 'conflict'));
    renderWithI18n(RuleTests);

    await fireEvent.click(await screen.findByRole('button', { name: 'Run tests' }));
    expect(
      await screen.findByText('All cases still route where they were pinned. Checked: 1.'),
    ).toBeTruthy();
    expect(screen.getByText('Passes')).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Run tests' }));

    expect(await screen.findByText('The rules could not be read')).toBeTruthy();
    expect(
      screen.queryByText('All cases still route where they were pinned. Checked: 1.'),
    ).toBeNull();
    expect(screen.queryByText('Passes')).toBeNull();
  });

  /** A case that moved is a failure, stated before the table that shows which one. */
  it('reports a run in which a case moved as a failure', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE]));
    vi.spyOn(api, 'runRuleTests').mockResolvedValue({
      total: 1,
      passed: 0,
      failed: 1,
      results: [
        {
          id: 't1',
          name: CASE.name,
          expected_category: 'anime',
          actual_category: 'standard',
          passed: false,
          matched_rule: null,
          error: null,
          error_key: null,
        },
      ],
    });
    renderWithI18n(RuleTests);

    await fireEvent.click(await screen.findByRole('button', { name: 'Run tests' }));

    const summary = await screen.findByText(
      'Cases no longer routing where they were pinned: 1 of 1.',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-danger')).toBe(true);
    expect(screen.getByText('Fails')).toBeTruthy();
  });

  it('offers no run while there is no case to run', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([]));
    renderWithI18n(RuleTests);

    await screen.findByText(
      "No pinned cases yet. Open any title's explanation and pin the decision: it becomes a case that fails if a later rule change moves it.",
    );
    expect(screen.getByRole('button', { name: 'Run tests' })).toBeDisabled();
  });
});

/** The cases come a page at a time, as many as the suite holds. */
describe('a suite longer than a page', () => {
  it('asks the server for the next page of cases', async () => {
    const list = vi
      .spyOn(api, 'getRuleTests')
      .mockResolvedValue(paginated([CASE], { total: 51, total_pages: 2 }));
    renderWithI18n(RuleTests);

    await userEvent.click(await screen.findByRole('button', { name: 'Next' }));

    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith({ page: 2, per_page: 50 }, expect.any(AbortSignal)),
    );
  });

  /** The last case of the last page gone, the page before it is the one left to show. */
  it('steps back a page once the last case of the last one is deleted', async () => {
    const list = vi
      .spyOn(api, 'getRuleTests')
      .mockResolvedValueOnce(paginated([CASE], { total: 51, total_pages: 2 }))
      .mockResolvedValueOnce(paginated([CASE], { page: 2, total: 51, total_pages: 2 }))
      .mockResolvedValueOnce(paginated([], { page: 2, total: 50, total_pages: 1 }))
      .mockResolvedValue(paginated([OTHER_CASE], { total: 50, total_pages: 1 }));
    vi.spyOn(api, 'deleteRuleTest').mockResolvedValue(undefined as never);
    renderWithI18n(RuleTests);
    await userEvent.click(await screen.findByRole('button', { name: 'Next' }));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));

    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete – Akira stays anime' }),
    );
    await answerConfirmation();

    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith({ page: 1, per_page: 50 }, expect.any(AbortSignal)),
    );
  });
});

/**
 * The list stays on screen while it reloads, since a skeleton would take the focus with the
 * rows, and the case in the deleted one's place takes the focus.
 */
describe('deleting a case', () => {
  const OTHER: RuleTest = { ...CASE, id: 't2', name: 'Perfect Blue stays anime' };

  async function deleteFirst(after: Promise<RuleTest[]>) {
    const list = vi
      .spyOn(api, 'getRuleTests')
      .mockResolvedValueOnce(paginated([CASE, OTHER]))
      .mockReturnValueOnce(after.then((cases) => paginated(cases)));
    vi.spyOn(api, 'deleteRuleTest').mockResolvedValue(undefined as never);
    renderWithI18n(RuleTests);

    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete – Akira stays anime' }),
    );
    await answerConfirmation();
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  }

  it('deletes nothing when the deletion is cancelled', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE, OTHER]));
    const remove = vi.spyOn(api, 'deleteRuleTest');
    renderWithI18n(RuleTests);

    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete – Perfect Blue stays anime' }),
    );
    await answerConfirmation(null);

    expect(remove).not.toHaveBeenCalled();
  });

  it('deletes the case whose row was used', async () => {
    await deleteFirst(new Promise(() => {}));

    expect(api.deleteRuleTest).toHaveBeenCalledExactlyOnceWith('t1');
  });

  it('keeps the list on screen while it reloads', async () => {
    await deleteFirst(new Promise(() => {}));

    expect(screen.getByRole('button', { name: 'Delete – Perfect Blue stays anime' })).toBeTruthy();
  });

  it('hands the focus to the case that took the place of the deleted one', async () => {
    await deleteFirst(Promise.resolve([OTHER]));

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Delete – Perfect Blue stays anime' }),
      ),
    );
  });

  it('hands the focus to the table once no case is left', async () => {
    vi.spyOn(api, 'getRuleTests')
      .mockResolvedValueOnce(paginated([CASE]))
      .mockResolvedValueOnce(paginated([]));
    vi.spyOn(api, 'deleteRuleTest').mockResolvedValue(undefined as never);
    renderWithI18n(RuleTests);

    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete – Akira stays anime' }),
    );
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('region', { name: 'Rule tests' })),
    );
  });
});

/** Running the cases records each verdict, and a reader is refused that as every write. */
describe('for a reader', () => {
  const offered = () =>
    ['Run tests', 'Delete – Akira stays anime'].filter((name) =>
      screen.queryByRole('button', { name }),
    );

  it('offers the owner the run and the delete, and a reader neither', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE]));
    const { unmount } = renderWithI18n(RuleTests);
    await screen.findByRole('row', { name: /Akira stays anime/ });
    expect(offered()).toEqual(['Run tests', 'Delete – Akira stays anime']);
    unmount();

    publishAccess(true);
    renderWithI18n(RuleTests);

    const row = await screen.findByRole('row', { name: /Akira stays anime/ });
    expect(within(row).getByText('anime')).toBeInTheDocument();
    expect(offered()).toEqual([]);
  });
});
