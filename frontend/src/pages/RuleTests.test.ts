import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { explanation, paginated } from '../test/fixtures';
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
  last_actual_category: null,
  source_media_id: null,
  subject: null,
};

/** The case, failing since a rule change sends its title to `standard`. */
const FAILING: RuleTest = {
  ...CASE,
  last_passed: false,
  last_actual_category: 'standard',
  last_checked_at: '2026-10-18 09:00:00',
};

/** A row's ⋯ menu, opened as a reader opens it, and one of its items. */
async function fromMenu(row: string, item: string) {
  await userEvent.click(await screen.findByRole('button', { name: `Actions – ${row}` }));
  await userEvent.click(screen.getByRole('menuitem', { name: item }));
}

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

afterEach(() => {
  vi.restoreAllMocks();
  window.history.replaceState({}, '', '/');
});

describe('RuleTests', () => {
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

  /** The question a failure raises, kept with the verdict so it survives the visit. */
  it('says where a failing test sends its title now, without a run on this visit', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([FAILING]));
    renderWithI18n(RuleTests);

    const row = await screen.findByRole('row', { name: /Akira stays anime/ });
    expect(within(row).getByText('Fails')).toBeInTheDocument();
    expect(within(row).getByText('now goes to standard')).toBeInTheDocument();
  });

  /** A test pinned from a title gone from the library has no explanation to open. */
  it('explains the title a test was pinned from, while the library holds it', async () => {
    vi.spyOn(api, 'getSources').mockResolvedValue({ providers: [], order: [] });
    vi.spyOn(api, 'getCategories').mockResolvedValue([]);
    vi.spyOn(api, 'getConditionCatalog').mockResolvedValue({
      match_modes: ['all', 'any'],
      conditions: [],
      score_scales: [],
    });
    const explain = vi.spyOn(api, 'explainTitle').mockResolvedValue(explanation());
    const list = vi
      .spyOn(api, 'getRuleTests')
      .mockResolvedValue(paginated([{ ...FAILING, source_media_id: 'm7' }, OTHER_CASE]));
    renderWithI18n(RuleTests);

    await userEvent.click(await screen.findByRole('button', { name: 'Why? – Akira stays anime' }));

    expect(explain).toHaveBeenCalledWith('m7', expect.any(AbortSignal));
    expect(screen.queryByRole('button', { name: 'Why? – Paprika stays anime' })).toBeNull();
    // Closed, the list is read again: the panel may have pinned another test.
    const loads = list.mock.calls.length;
    await userEvent.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Dismiss' }),
    );
    await waitFor(() => expect(list.mock.calls.length).toBeGreaterThan(loads));
  });

  /** The usual failure follows a deliberate rule change, which the test is then told to accept. */
  it('expects where the title goes now in one press, asked first with both categories', async () => {
    vi.spyOn(api, 'getRuleTests')
      .mockResolvedValueOnce(paginated([FAILING]))
      .mockResolvedValue(
        paginated([{ ...FAILING, expected_category: 'standard', last_passed: true }]),
      );
    const expected = vi.spyOn(api, 'setRuleTestExpectation').mockResolvedValue(FAILING);
    renderWithI18n(RuleTests);

    await userEvent.click(
      await screen.findByRole('button', { name: 'Expect standard – Akira stays anime' }),
    );
    expect(expected).not.toHaveBeenCalled();
    expect(await answerConfirmation()).toBe(
      'The test "Akira stays anime" will expect "standard" from now on, instead of "anime".',
    );

    await waitFor(() => expect(expected).toHaveBeenCalledWith('t1', 'standard'));
    expect(
      await screen.findByText('The test "Akira stays anime" now expects "standard".'),
    ).toBeTruthy();
    expect(await screen.findByText('Passes')).toBeTruthy();
  });

  it('changes no expectation when the question is declined, and offers none on a pass', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(
      paginated([FAILING, { ...OTHER_CASE, last_passed: true, last_actual_category: 'anime' }]),
    );
    const expected = vi.spyOn(api, 'setRuleTestExpectation');
    renderWithI18n(RuleTests);

    await userEvent.click(
      await screen.findByRole('button', { name: 'Expect standard – Akira stays anime' }),
    );
    await answerConfirmation(null);

    expect(expected).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: /^Expect .* – Paprika stays anime$/ })).toBeNull();
  });

  /** The warning about failing tests leads here, to the failures it counts. */
  it('lists the failing tests alone when sent to them, and all of them on request', async () => {
    window.history.replaceState({}, '', '/rule-tests?failing=1');
    const list = vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([FAILING]));
    renderWithI18n(RuleTests);

    const only = await screen.findByRole('checkbox', { name: 'Failing only' });
    expect(only).toBeChecked();
    expect(list).toHaveBeenLastCalledWith(
      { page: 1, per_page: 50, failing: true },
      expect.any(AbortSignal),
    );
    expect(window.location.search).toBe('?failing=1');

    await userEvent.click(only);

    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith({ page: 1, per_page: 50 }, expect.any(AbortSignal)),
    );
    expect(window.location.search).toBe('');
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

  it('takes the previous run off screen when the next one fails', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE]));
    vi.spyOn(api, 'runRuleTests')
      .mockResolvedValueOnce(PASSED)
      .mockRejectedValueOnce(new ApiError('The rules could not be read', 409, 'conflict'));
    renderWithI18n(RuleTests);

    await fireEvent.click(await screen.findByRole('button', { name: 'Run tests' }));
    expect(
      await screen.findByText('All tests still route where they were pinned. Checked: 1.'),
    ).toBeTruthy();
    expect(screen.getByText('Passes')).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Run tests' }));

    expect(await screen.findByText('The rules could not be read')).toBeTruthy();
    expect(
      screen.queryByText('All tests still route where they were pinned. Checked: 1.'),
    ).toBeNull();
    expect(screen.queryByText('Passes')).toBeNull();
  });

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
      'Tests no longer routing where they were pinned: 1 of 1.',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-danger')).toBe(true);
    expect(screen.getByText('Fails')).toBeTruthy();
  });

  it('offers no run while there is no case to run', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([]));
    renderWithI18n(RuleTests);

    await screen.findByText(
      "No pinned tests yet. Open any title's explanation and pin the decision: it becomes a test that fails if a later rule change moves it.",
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

    await fromMenu('Akira stays anime', 'Delete');
    await answerConfirmation();

    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith({ page: 1, per_page: 50 }, expect.any(AbortSignal)),
    );
  });
});

describe('deleting a case', () => {
  const OTHER: RuleTest = { ...CASE, id: 't2', name: 'Perfect Blue stays anime' };

  async function deleteFirst(after: Promise<RuleTest[]>) {
    const list = vi
      .spyOn(api, 'getRuleTests')
      .mockResolvedValueOnce(paginated([CASE, OTHER]))
      .mockReturnValueOnce(after.then((cases) => paginated(cases)));
    vi.spyOn(api, 'deleteRuleTest').mockResolvedValue(undefined as never);
    renderWithI18n(RuleTests);

    await fromMenu('Akira stays anime', 'Delete');
    await answerConfirmation();
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  }

  it('deletes nothing when the deletion is cancelled', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE, OTHER]));
    const remove = vi.spyOn(api, 'deleteRuleTest');
    renderWithI18n(RuleTests);

    await fromMenu('Perfect Blue stays anime', 'Delete');
    await answerConfirmation(null);

    expect(remove).not.toHaveBeenCalled();
  });

  it('deletes the case whose row was used', async () => {
    await deleteFirst(new Promise(() => {}));

    expect(api.deleteRuleTest).toHaveBeenCalledExactlyOnceWith('t1');
  });

  it('keeps the list on screen while it reloads', async () => {
    await deleteFirst(new Promise(() => {}));

    expect(screen.getByRole('button', { name: 'Actions – Perfect Blue stays anime' })).toBeTruthy();
  });

  it('hands the focus to the case that took the place of the deleted one', async () => {
    await deleteFirst(Promise.resolve([OTHER]));

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Actions – Perfect Blue stays anime' }),
      ),
    );
  });

  it('hands the focus to the table once no case is left', async () => {
    vi.spyOn(api, 'getRuleTests')
      .mockResolvedValueOnce(paginated([CASE]))
      .mockResolvedValueOnce(paginated([]));
    vi.spyOn(api, 'deleteRuleTest').mockResolvedValue(undefined as never);
    renderWithI18n(RuleTests);

    await fromMenu('Akira stays anime', 'Delete');
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('region', { name: 'Rule tests' })),
    );
  });
});

/** Running the cases records each verdict, and a reader is refused that as every write. */
describe('for a reader', () => {
  const offered = () =>
    ['Run tests', 'Actions – Akira stays anime'].filter((name) =>
      screen.queryByRole('button', { name }),
    );

  it('offers the owner the run and the delete, and a reader neither', async () => {
    vi.spyOn(api, 'getRuleTests').mockResolvedValue(paginated([CASE]));
    const { unmount } = renderWithI18n(RuleTests);
    await screen.findByRole('row', { name: /Akira stays anime/ });
    expect(offered()).toEqual(['Run tests', 'Actions – Akira stays anime']);
    unmount();

    publishAccess(true);
    renderWithI18n(RuleTests);

    const row = await screen.findByRole('row', { name: /Akira stays anime/ });
    expect(within(row).getByText('anime')).toBeInTheDocument();
    expect(offered()).toEqual([]);
  });
});
