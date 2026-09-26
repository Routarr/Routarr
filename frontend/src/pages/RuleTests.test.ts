import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { ApiError, api } from '../api/client';
import type { RuleTest, RuleTestRun } from '../api/types';
import RuleTests from './RuleTests.svelte';
import { answerConfirmation } from '../test/confirm';

const STRINGS = {
  RuleTests: 'Rule tests',
  RunRuleTests: 'Run the tests',
  RuleTestsPassed: 'Every case holds. Checked: {total}',
  RuleTestsFailed: 'Cases that moved: {failed} of {total}',
  Passed: 'Passed',
  Failed: 'Failed',
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

afterEach(() => vi.restoreAllMocks());

describe('RuleTests', () => {
  /**
   * A run that fails leaves nothing of the run before it. Kept, the earlier
   * summary and verdicts read as the answer to the run that was just refused.
   */
  it('takes the previous run off screen when the next one fails', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue([CASE]);
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
    vi.spyOn(api, 'getRuleTests').mockResolvedValue([CASE]);
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

  /** Every screen frames an empty list the way it frames a full one. */
  it('says there is no case inside the table, as the other screens do', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue([]);
    renderWithI18n(RuleTests, { strings: { ...STRINGS, NoRuleTests: 'No pinned case yet' } });

    const empty = await screen.findByText('No pinned case yet');
    expect(empty.closest('.card')).not.toBeNull();
    expect(screen.getByRole('table', { name: 'Rule tests' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Run the tests' })).toBeDisabled();
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
      .mockResolvedValueOnce([CASE, OTHER])
      .mockReturnValueOnce(after);
    vi.spyOn(api, 'deleteRuleTest').mockResolvedValue(undefined as never);
    renderWithI18n(RuleTests, { strings: STRINGS });

    await userEvent.click(
      await screen.findByRole('button', { name: 'Delete – Akira stays anime' }),
    );
    await answerConfirmation();
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  }

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
    vi.spyOn(api, 'getRuleTests').mockResolvedValueOnce([CASE]).mockResolvedValueOnce([]);
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
