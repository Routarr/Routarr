import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { paginated } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import type { RuleTest, RuleTestRun } from '../api/types';
import RuleTests from './RuleTests.svelte';
import { answerConfirmation } from '../test/confirm';

const STRINGS = {
  RuleTests: 'Rule tests',
  RunRuleTests: 'Run the tests',
  RunningRuleTests: 'Running the tests…',
  RuleTestsPassed: 'Every case holds. Checked: {total}',
  RuleTestsFailed: 'Cases that moved: {failed} of {total}',
  Passed: 'Passed',
  Failed: 'Failed',
  Next: 'Next',
};

const CASE: RuleTest = {
  id: 't1',
  name: 'Akira stays anime',
  media_type: 'movie',
  expected_category: 'anime',
  source_media_title: 'Akira',
  evaluated_at: '2026-09-01 10:00:00',
  created_at: '2026-09-01 10:00:00',
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
    },
  ],
};

const OTHER_CASE: RuleTest = { ...CASE, id: 't3', name: 'Paprika stays anime' };

afterEach(() => vi.restoreAllMocks());

describe('RuleTests', () => {
  /** A run takes the whole library: the button says it is running, not only spins. */
  it('says the tests are running while they run', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE]));
    vi.spyOn(api, 'runRuleTests').mockReturnValue(new Promise(() => {}));
    renderWithI18n(RuleTests, { strings: STRINGS });

    await fireEvent.click(await screen.findByRole('button', { name: 'Run the tests' }));

    const running = await screen.findByRole('button', { name: 'Running the tests…' });
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
    renderWithI18n(RuleTests, { strings: STRINGS });

    await fireEvent.click(await screen.findByRole('button', { name: 'Run the tests' }));
    expect(await screen.findByText('Every case holds. Checked: 1')).toBeTruthy();
    expect(screen.getByText('Passed')).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Run the tests' }));

    expect(await screen.findByText('The rules could not be read')).toBeTruthy();
    expect(screen.queryByText('Every case holds. Checked: 1')).toBeNull();
    expect(screen.queryByText('Passed')).toBeNull();
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
        },
      ],
    });
    renderWithI18n(RuleTests, { strings: STRINGS });

    await fireEvent.click(await screen.findByRole('button', { name: 'Run the tests' }));

    const summary = await screen.findByText('Cases that moved: 1 of 1');
    expect(summary.closest('.banner')?.classList.contains('banner-danger')).toBe(true);
    expect(screen.getByText('Failed')).toBeTruthy();
  });

  it('offers no run while there is no case to run', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([]));
    renderWithI18n(RuleTests, { strings: { ...STRINGS, NoRuleTests: 'No pinned case yet' } });

    await screen.findByText('No pinned case yet');
    expect(screen.getByRole('button', { name: 'Run the tests' })).toBeDisabled();
  });
});

/** The cases come a page at a time, as many as the suite holds. */
describe('a suite longer than a page', () => {
  it('asks the server for the next page of cases', async () => {
    const list = vi
      .spyOn(api, 'getRuleTests')
      .mockResolvedValue(paginated([CASE], { total: 51, total_pages: 2 }));
    renderWithI18n(RuleTests, { strings: STRINGS });

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
    renderWithI18n(RuleTests, { strings: STRINGS });
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
 * A deleted case takes its row away. Swapped for a skeleton while the list
 * reloads, the other rows would take the focus with them, so the list stays,
 * and what takes the deleted row's place takes the focus.
 */
describe('deleting a case', () => {
  const OTHER: RuleTest = { ...CASE, id: 't2', name: 'Perfect Blue stays anime' };

  async function deleteFirst(after: Promise<RuleTest[]>) {
    const list = vi
      .spyOn(api, 'getRuleTests')
      .mockResolvedValueOnce(paginated([CASE, OTHER]))
      .mockReturnValueOnce(after.then((cases) => paginated(cases)));
    vi.spyOn(api, 'deleteRuleTest').mockResolvedValue(undefined as never);
    renderWithI18n(RuleTests, { strings: STRINGS });

    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete – Akira stays anime' }),
    );
    await answerConfirmation();
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  }

  it('deletes nothing when the deletion is cancelled', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE, OTHER]));
    const remove = vi.spyOn(api, 'deleteRuleTest');
    renderWithI18n(RuleTests, { strings: STRINGS });

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
    renderWithI18n(RuleTests, { strings: STRINGS });

    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete – Akira stays anime' }),
    );
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('region', { name: 'Rule tests' })),
    );
  });
});
