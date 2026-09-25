import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { ApiError, api } from '../api/client';
import type { RuleTest, RuleTestRun } from '../api/types';
import RuleTests from './RuleTests.svelte';

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
});
