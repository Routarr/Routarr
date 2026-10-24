import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { category, decision, job, paginated, rule } from '../test/fixtures';
import userEvent from '@testing-library/user-event';
import { ApiError, api, type Following } from '../api/client';
import type { Decision, SimulationResult } from '../api/types';
import Simulation from './Simulation.svelte';
import { answerConfirmation } from '../test/confirm';
import { dropFocus } from '../test/focus';
import { statusRevision } from '../lib/status.svelte';
import { publishAccess } from '../lib/access.svelte';

/**
 * The screen that writes. It enforces the executor's guards again before any request, and the
 * end-to-end journeys drive only the happy path, where a missing guard cannot be seen.
 */

/** What each run stored, as `/decisions` lists it under the run's id. */
const RUNS = new Map<string, Decision[]>();

/** A stored run's report: its counts alone, its proposals left for `/decisions` to list. */
function simulation(decisions: Decision[]): SimulationResult {
  const id = `s${RUNS.size + 1}`;
  RUNS.set(id, decisions);
  return {
    simulation_id: id,
    capacity: [],
    returned: 0,
    decisions: [],
    overrides_applied: 0,
    elapsed_ms: 4,
    total_media: decisions.length,
    moves_required: decisions.filter((d) => d.action === 'move').length,
    already_correct: 0,
    no_category_match: 0,
    skipped_unmapped: 0,
    excluded_by_rule: 0,
  };
}

/** What the server answers Apply all with until its batch question is answered. */
const batchQuestion = () =>
  new ApiError('1 item will move.', 409, 'confirmation_required', null, 'batch');

const batchReport = {
  candidates: 1,
  applied: 1,
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

/** What the server answers an apply of one selected move that it made. */
const applied = {
  requested: 1,
  applied: 1,
  failed: 0,
  skipped: 0,
  moving: 0,
  superseded: 0,
  stopped: null,
  errors: [],
};

/** The decisions a run stored, or those left pending, that `params` keeps. */
function listed(pending: Decision[], params: Record<string, unknown> = {}) {
  const run = typeof params.simulation_id === 'string' ? RUNS.get(params.simulation_id) : null;
  return (run ?? pending).filter(
    (d) =>
      (!params.status || d.status === params.status) &&
      (!params.action || d.action === params.action),
  );
}

/** Render and wait for the pending-decisions fetch the page makes on mount. */
async function show(pending: Decision[]) {
  vi.spyOn(api, 'getDecisions').mockImplementation(async (params) =>
    paginated(listed(pending, params)),
  );
  vi.spyOn(api, 'countDecisions').mockImplementation(
    async (params) => listed(pending, params).length,
  );
  const view = renderWithI18n(Simulation);
  await screen.findByRole('heading', { name: 'Simulation' });
  return view;
}

// No run is going on when the screen opens, unless a test says otherwise, and nothing to filter.
beforeEach(() => {
  vi.spyOn(api, 'getTasks').mockResolvedValue(paginated([]));
  vi.spyOn(api, 'getSettings').mockResolvedValue({ batch_limit: '50' });
  vi.spyOn(api, 'getInstances').mockResolvedValue([]);
  vi.spyOn(api, 'getCategories').mockResolvedValue([]);
  vi.spyOn(api, 'getRootFolders').mockResolvedValue([]);
  vi.spyOn(api, 'getRules').mockResolvedValue([]);
});

/** A field of the filter panel, opened first as a reader opens it. */
function filterField(label: string): HTMLElement {
  const toggle = screen.getByRole('button', { name: /^Filters/ });
  if (toggle.getAttribute('aria-expanded') !== 'true') toggle.click();
  return screen.getByLabelText(label, { selector: 'select, input' });
}

afterEach(() => vi.restoreAllMocks());

describe('what the screen shows', () => {
  /**
   * The dashboard sends the reader here with a count to review, which the empty state would
   * contradict, so a failed request renders a banner.
   */
  it('says the pending list could not be loaded rather than pretending it is empty', async () => {
    vi.spyOn(api, 'getDecisions').mockRejectedValue(new ApiError('later', 503, 'unavailable'));
    renderWithI18n(Simulation);

    expect(await screen.findByRole('alert')).toHaveTextContent('later');
  });

  it('asks the pending list again when the reader retries', async () => {
    const pendingDecision = decision({ id: 'd-retry', media_title: 'Retried' });
    vi.spyOn(api, 'getDecisions')
      .mockRejectedValueOnce(new ApiError('later', 503, 'unavailable'))
      .mockResolvedValue(paginated([pendingDecision]));
    renderWithI18n(Simulation);

    await fireEvent.click(await screen.findByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('Retried')).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('holds the table open while the pending list loads', async () => {
    vi.spyOn(api, 'getDecisions').mockReturnValue(new Promise(() => {}));
    renderWithI18n(Simulation);

    expect(await screen.findByText('Loading…')).toHaveAttribute('role', 'status');
    expect(screen.getByRole('table', { name: 'Simulation' })).toBeTruthy();
    expect(
      screen.queryByText(
        'Run a simulation to see what Routarr would do. Nothing is written until you apply.',
      ),
    ).toBeNull();
  });

  /**
   * Decisions a previous pass left are stored, so the screen shows them before any run, or the
   * dashboard's count would land on an empty state.
   */
  it('shows the decisions a previous pass left pending, before any run', async () => {
    await show([decision({ media_title: 'Akira' })]);

    expect(await screen.findByText('Akira')).toBeTruthy();
  });

  it('replaces the pending list with a fresh run rather than showing both', async () => {
    await show([decision({ media_title: 'Akira' })]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision({ media_title: 'Totoro' })]),
    );

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    await waitFor(() => expect(screen.getByText('Totoro')).toBeTruthy());
    expect(screen.queryByText('Akira')).toBeNull();
  });

  it('tells the shell a simulation ran', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    const before = statusRevision();

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /**
   * A run pre-selects its moves so the common case is one click, and never a skip, which has no
   * target folder and which the executor can only refuse.
   */
  it('pre-selects the moves and leaves the skips alone', async () => {
    await show([]);
    const moving = decision({ media_title: 'Akira' });
    const skip = decision({ media_title: 'Dune', action: 'skip', target_root_folder: null });
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([moving, skip]));
    const apply = vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await waitFor(() => expect(screen.getByText('Akira')).toBeTruthy());

    const boxes = screen.getAllByRole('checkbox', { name: /select the move for/i });
    expect(boxes).toHaveLength(1);
    expect((boxes[0] as HTMLInputElement).checked).toBe(true);

    // Asserted on the request: a skip has no checkbox to show a wrong pre-selection.
    await fireEvent.click(screen.getByRole('button', { name: /apply selected/i }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(nthCall(apply)[0]).toEqual([moving.id]);
  });
});

describe('keeping a title where it is', () => {
  it('pins a pending proposal to its folder category, and reloads the list', async () => {
    const kept = vi.spyOn(api, 'keepTitle').mockResolvedValue({} as never);
    await show([decision({ media_id: 'm7', media_title: 'Akira', current_category: 'standard' })]);
    const before = statusRevision();
    const loads = vi.mocked(api.getDecisions).mock.calls.length;

    await fireEvent.click(await screen.findByRole('button', { name: 'Keep here – Akira' }));

    expect(kept).toHaveBeenCalledWith('m7');
    expect(
      await screen.findByText('Kept in "standard": an exception holds the title there.'),
    ).toBeTruthy();
    expect(statusRevision()).toBeGreaterThan(before);
    expect(vi.mocked(api.getDecisions).mock.calls.length).toBeGreaterThan(loads);
  });

  it('offers no keep for a title whose folder leads to no category', async () => {
    await show([decision({ media_title: 'Akira', current_category: null })]);

    await screen.findByText('Akira');
    expect(screen.queryByRole('button', { name: 'Keep here – Akira' })).toBeNull();
  });
});

/**
 * Moving the files on disk is the one choice an apply cannot take back, and it travels as a
 * flag nothing else on screen shows.
 */
describe('the file move', () => {
  it('is left off unless ticked', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    const apply = vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(nthCall(apply)[1]).toBe(false);
  });

  it('goes with the selected moves once ticked', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    const apply = vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByLabelText(/^Move the files on disk too/));
    await fireEvent.click(screen.getByRole('button', { name: /apply selected/i }));

    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(nthCall(apply)[1]).toBe(true);
  });

  it('goes with Apply all once ticked, and stays off without it', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    const applyAll = vi.spyOn(api, 'applyAllDecisions').mockResolvedValue(batchReport);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));
    await waitFor(() => expect(applyAll).toHaveBeenCalledTimes(1));
    expect(nthCall(applyAll)[2]).toBe(false);

    await fireEvent.click(await screen.findByLabelText(/^Move the files on disk too/));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));
    await waitFor(() => expect(applyAll).toHaveBeenCalledTimes(2));
    expect(nthCall(applyAll, 1)[2]).toBe(true);
  });
});

describe('what the screen refuses to do', () => {
  it('applies nothing when nothing is selected', async () => {
    const apply = vi.spyOn(api, 'applyDecisions');
    // Pending decisions are shown but not pre-selected: only a run selects.
    await show([decision({ media_title: 'Akira' })]);

    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    // Settled first: a `waitFor` around a negative passes before the handler has run.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(apply).not.toHaveBeenCalled();
  });

  it('applies nothing when the confirmation is declined', async () => {
    const applyAll = vi.spyOn(api, 'applyAllDecisions').mockRejectedValue(batchQuestion());
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));

    // The question says how many, so a user who miscounted stops here.
    expect(await answerConfirmation(null)).toMatch(/1/);
    // Asked, and never answered: the one request carried no answer.
    expect(applyAll).toHaveBeenCalledTimes(1);
    expect(nthCall(applyAll)[3]).toEqual([]);
  });

  /**
   * Past the configured threshold the backend asks for a second pass. A red banner would say the
   * moves went wrong when nothing happened, so it is a question.
   */
  it('turns the threshold refusal into a question, not an error', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    const refusal = new ApiError(
      'Above the threshold',
      409,
      'confirmation_required',
      null,
      'threshold',
    );
    const apply = vi
      .spyOn(api, 'applyDecisions')
      .mockRejectedValueOnce(refusal)
      .mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    // The refusal's own message is the only place the threshold is named.
    expect(await answerConfirmation()).toContain('Above the threshold');

    // Applied again naming only the guardrail answered, so a second refusal is asked too.
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(nthCall(apply)[2]).toEqual([]);
    expect(nthCall(apply, 1)[2]).toEqual(['threshold']);
  });

  it('lets the server ask its own question before Apply all writes', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    const question = new ApiError(
      "1 item will move.\n\n'/mnt/nas' is not answering.",
      409,
      'confirmation_required',
      null,
      'batch',
      ['unreachable'],
    );
    const applyAll = vi
      .spyOn(api, 'applyAllDecisions')
      .mockRejectedValueOnce(question)
      .mockResolvedValue(batchReport);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));

    expect(await answerConfirmation()).toContain("'/mnt/nas' is not answering.");
    await waitFor(() => expect(applyAll).toHaveBeenCalledTimes(2));
    // The one question answers what it states, so every name goes back.
    expect(nthCall(applyAll)[3]).toEqual([]);
    expect(nthCall(applyAll, 1)[3]).toEqual(['batch', 'unreachable']);
  });

  it('sends a skipped item to the screen that maps its category', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue({
      ...simulation([decision()]),
      skipped_unmapped: 2,
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    expect(
      await screen.findByText(
        'Items matching a category with no root folder on their instance: 2. They will be skipped. Map the category on the Categories and folders screen.',
      ),
    ).toBeTruthy();
  });

  it('says that Apply all reaches past the rows on screen', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    const button = await screen.findByRole('button', { name: /apply all/i });
    expect(button).toHaveAccessibleDescription(
      'Applies every move this run proposed, including those beyond the displayed list, in batches.',
    );
    expect(
      screen.getByText(
        'Applies every move this run proposed, including those beyond the displayed list, in batches.',
      ),
    ).toBeInTheDocument();
  });

  it('stops following an apply once the screen closes', async () => {
    const applyDecisions = vi.spyOn(api, 'applyDecisions').mockReturnValue(new Promise(() => {}));
    const { unmount } = await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));
    await waitFor(() => expect(applyDecisions).toHaveBeenCalled());
    const signal = nthCall(applyDecisions)[3]?.signal;
    expect(signal?.aborted).toBe(false);

    unmount();
    expect(signal?.aborted).toBe(true);
  });

  /** A second click while the confirmed apply writes would start another. */
  it('holds Apply and Run while the confirmed apply is running', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    vi.spyOn(api, 'applyDecisions')
      .mockRejectedValueOnce(
        new ApiError('Above the threshold', 409, 'confirmation_required', null, 'threshold'),
      )
      .mockReturnValue(new Promise(() => {}));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));
    await answerConfirmation();

    await waitFor(() => expect(api.applyDecisions).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('button', { name: /run simulation/i })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Applying…' })).toBeDisabled();
    expect(screen.getByRole('button', { name: /apply all/i })).toBeDisabled();
  });
});

describe('what the screen says after applying', () => {
  it('takes the previous report off screen when the next apply is refused', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision({ media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'applyDecisions')
      .mockResolvedValueOnce(applied)
      .mockRejectedValueOnce(new ApiError('The Arr refused the move', 502, 'bad_gateway'));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));
    expect(await screen.findByText('Decisions applied: 1 of 1.')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    expect(await screen.findByText('The Arr refused the move')).toBeTruthy();
    expect(screen.queryByText('Decisions applied: 1 of 1.')).toBeNull();
  });

  it('shows only the latest report when one apply follows another', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision({ media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'applyAllDecisions')
      .mockRejectedValueOnce(batchQuestion())
      .mockResolvedValue(batchReport);
    vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));
    await answerConfirmation();
    expect(await screen.findByText('Decisions applied: 1 of 1. Batches: 1.')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    expect(await screen.findByText('Decisions applied: 1 of 1.')).toBeTruthy();
    expect(screen.queryByText('Decisions applied: 1 of 1. Batches: 1.')).toBeNull();
  });

  it('reports an apply in which every decision failed as a failure', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision({ media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'applyDecisions').mockResolvedValue({
      requested: 1,
      applied: 0,
      failed: 1,
      skipped: 0,
      moving: 0,
      superseded: 0,
      stopped: null,
      errors: [{ decision_id: 'd1', media_title: 'Akira', message: 'The Arr refused the move' }],
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    const summary = await screen.findByText('Decisions applied: 0 of 1.');
    expect(summary.closest('[role]')?.getAttribute('role')).toBe('alert');
    expect(screen.getByText('Akira: The Arr refused the move')).toBeTruthy();
  });

  it('keeps the report of an apply whose refresh failed', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation')
      .mockResolvedValueOnce(simulation([decision({ media_title: 'Akira' })]))
      .mockRejectedValueOnce(new ApiError('The library could not be read', 409, 'conflict'));
    vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    expect(await screen.findByText('The library could not be read')).toBeTruthy();
    expect(screen.getByText('Decisions applied: 1 of 1.')).toBeTruthy();
  });

  it('tells the shell of an apply whose refresh failed', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation')
      .mockResolvedValueOnce(simulation([decision({ media_title: 'Akira' })]))
      .mockRejectedValueOnce(new ApiError('The library could not be read', 409, 'conflict'));
    vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);
    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    const apply = await screen.findByRole('button', { name: /apply selected/i });
    const before = statusRevision();

    await fireEvent.click(apply);

    await screen.findByText('The library could not be read');
    expect(statusRevision()).toBeGreaterThan(before);
  });

  /**
   * Part done and part failed is neither a success nor a failure: red over 49 moves made says
   * nothing was done, green hides the one that was not.
   */
  it('reports an Apply all stopped after some moves as a partial result', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision({ media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'applyAllDecisions')
      .mockRejectedValueOnce(batchQuestion())
      .mockResolvedValue({
        candidates: 50,
        applied: 49,
        failed: 1,
        skipped: 0,
        batches_run: 1,
        batches_planned: 1,
        moving: 0,
        superseded: 0,
        stopped_early: true,
        stopped: 'refused',
        errors: [{ decision_id: 'd1', media_title: 'Akira', message: 'The Arr refused the move' }],
      });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));
    await answerConfirmation();

    const summary = await screen.findByText(
      'Stopped after a failure. Applied: 49 of 50. Batches run: 1 of 1. The rest is still pending.',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(
      screen.getByText(
        'Stopped: the Arr refused every move of a batch. The rest was not attempted.',
      ),
    ).toBeTruthy();
  });

  it('says what the Arr is still moving and what was replaced', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision({ id: 'd1' })]));
    vi.spyOn(api, 'applyDecisions').mockResolvedValue({
      requested: 3,
      applied: 1,
      failed: 0,
      skipped: 0,
      moving: 1,
      superseded: 1,
      stopped: null,
      errors: [],
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    const summary = await screen.findByText('Decisions applied: 1 of 3.');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(
      screen.getByText('Still moving in the Arr: 1. The next sync confirms each move.'),
    ).toBeTruthy();
    expect(screen.getByText('Replaced since by other moves, not applied: 1.')).toBeTruthy();
  });

  it('stops the apply it follows', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision({ id: 'd1' })]));
    let finish: () => void = () => undefined;
    vi.spyOn(api, 'applyDecisions').mockImplementation(
      (_ids, _files, _confirm, following?: Following) => {
        following?.onProgress?.(job({ id: 'j-apply', kind: 'apply', status: 'running' }));
        return new Promise((resolve) => {
          finish = () =>
            resolve({
              requested: 1,
              applied: 0,
              failed: 0,
              skipped: 0,
              moving: 0,
              superseded: 0,
              stopped: 'cancelled',
              errors: [],
            });
        });
      },
    );
    const cancelTask = vi.spyOn(api, 'cancelTask').mockResolvedValue(undefined);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    finish();

    expect(cancelTask).toHaveBeenCalledWith('j-apply');
    expect(await screen.findByText('Stopped on request: the rest was not attempted.')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
  });

  /** One whole sentence per case: a clause appended to a translated one is English grammar. */
  it('says how many proposals had gone stale in the same sentence as the count', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision({ id: 'd1' })]));
    vi.spyOn(api, 'applyDecisions').mockResolvedValue({
      requested: 2,
      applied: 1,
      failed: 0,
      skipped: 1,
      moving: 0,
      superseded: 0,
      stopped: null,
      errors: [],
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    expect(
      await screen.findByText('Decisions applied: 1 of 2, no longer actionable: 1.'),
    ).toBeTruthy();
  });

  it('asks about the files in the same sentence as the count', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    vi.spyOn(api, 'applyDecisions').mockRejectedValue(batchQuestion());

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByLabelText(/^Move the files on disk too/));
    await fireEvent.click(screen.getByRole('button', { name: /apply selected/i }));

    expect(await answerConfirmation(null)).toContain(
      'Items to be re-pointed in Radarr/Sonarr, their files moved on disk too: 1.',
    );
  });

  it('reports an apply of selected moves that partly failed as a partial result', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([
        decision({ id: 'd1', media_title: 'Akira' }),
        decision({ id: 'd2', media_title: 'Heat' }),
      ]),
    );
    vi.spyOn(api, 'applyDecisions').mockResolvedValue({
      requested: 2,
      applied: 1,
      failed: 1,
      skipped: 0,
      moving: 0,
      superseded: 0,
      stopped: null,
      errors: [{ decision_id: 'd2', media_title: 'Heat', message: 'The Arr refused the move' }],
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    const summary = await screen.findByText('Decisions applied: 1 of 2.');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
  });

  /** A new run starts a new review, and the report of the last apply belongs to the old one. */
  it('takes the report of the last apply off screen when a new simulation runs', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision({ media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));
    expect(await screen.findByText('Decisions applied: 1 of 1.')).toBeTruthy();
    const rerun = await screen.findByRole('button', { name: /run simulation/i });
    await waitFor(() => expect((rerun as HTMLButtonElement).disabled).toBe(false));

    await fireEvent.click(rerun);

    await waitFor(() => expect(screen.queryByText('Decisions applied: 1 of 1.')).toBeNull());
  });

  it('takes the failed moves off screen with the outcome they explain', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision({ media_title: 'Akira' })]),
    );
    vi.spyOn(api, 'applyDecisions').mockResolvedValue({
      requested: 1,
      applied: 0,
      failed: 1,
      skipped: 0,
      moving: 0,
      superseded: 0,
      stopped: null,
      errors: [{ decision_id: 'd1', media_title: 'Akira', message: 'The Arr refused the move' }],
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));
    await screen.findByText('Akira: The Arr refused the move');

    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));

    expect(screen.queryByText('Akira: The Arr refused the move')).toBeNull();
  });

  it.each([
    ['Apply selected', /apply selected/i],
    ['Apply all', /apply all/i],
  ])('hands the focus on once %s ends', async (_, name) => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    vi.spyOn(api, 'applyDecisions').mockImplementation(async () => {
      await held;
      return applied;
    });
    vi.spyOn(api, 'applyAllDecisions').mockImplementation(async () => {
      await held;
      return batchReport;
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    const button = await screen.findByRole('button', { name });
    button.focus();
    await fireEvent.click(button);
    await waitFor(() => expect(button).toBeDisabled());
    dropFocus();
    release();
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /run simulation/i })).toBeEnabled(),
    );

    await waitFor(() => expect(document.activeElement).not.toBe(document.body));
  });
});

/** A run of any size is followed through its task, so no request bound cuts it. */
describe('a run followed through its task', () => {
  it('shows how far the run has gone while it is followed', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockImplementation((_, following?: Following) => {
      following?.onProgress?.(job({ progress_current: 50, progress_total: 200 }));
      return new Promise(() => {});
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    const bar = await screen.findByRole('progressbar', { name: 'Evaluating rules…' });
    expect(bar).toHaveAttribute('aria-valuenow', '50');
    expect(bar).toHaveAttribute('aria-valuemax', '200');
  });

  it('follows a run already going when the screen opens, and lists what it proposed', async () => {
    vi.spyOn(api, 'getTasks').mockResolvedValue(paginated([job({ id: 'j9', kind: 'simulate' })]));
    let finish: (result: SimulationResult) => void = () => {};
    const follow = vi
      .spyOn(api, 'followTask')
      .mockReturnValue(new Promise((resolve) => (finish = resolve as never)));
    await show([]);

    await waitFor(() => expect(follow).toHaveBeenCalledWith('j9', expect.anything()));
    expect(screen.getByRole('button', { name: 'Evaluating rules…' })).toBeDisabled();

    finish(simulation([decision({ media_title: 'Totoro' })]));

    expect(await screen.findByText('Totoro')).toBeTruthy();
    expect(screen.getByRole('button', { name: /run simulation/i })).toBeEnabled();
  });

  it('stops following when the screen closes, and leaves the run to go on', async () => {
    let signal: AbortSignal | undefined;
    vi.spyOn(api, 'runSimulation').mockImplementation((_, following?: Following) => {
      signal = following?.signal;
      return new Promise(() => {});
    });
    vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated([]));
    const view = renderWithI18n(Simulation);
    await fireEvent.click(await screen.findByRole('button', { name: /run simulation/i }));
    expect(signal?.aborted).toBe(false);

    view.unmount();

    expect(signal?.aborted).toBe(true);
  });

  it('pages through what the run proposed', async () => {
    const getDecisions = vi
      .spyOn(api, 'getDecisions')
      .mockResolvedValue(paginated([decision()], { total_pages: 2, total: 250 }));
    renderWithI18n(Simulation);
    const run = simulation([decision()]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(run);

    await fireEvent.click(await screen.findByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Next' }));

    await waitFor(() =>
      expect(getDecisions).toHaveBeenLastCalledWith(
        expect.objectContaining({ simulation_id: run.simulation_id, page: 2 }),
        expect.any(AbortSignal),
      ),
    );
  });
});

describe('narrowing what is reviewed and applied', () => {
  it('preselects no more moves than the batch limit, and says where the selection stops', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue({ batch_limit: '2' });
    await show([]);
    const moves = [decision(), decision(), decision()];
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation(moves));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    expect(await screen.findByRole('button', { name: 'Apply selected (2)' })).toBeTruthy();
    expect(
      screen.getByText('The selection stops at the batch limit. Moves at a time: 2.'),
    ).toBeTruthy();
    await fireEvent.click(screen.getByRole('checkbox', { name: 'Select every proposed move' }));
    await fireEvent.click(screen.getByRole('checkbox', { name: 'Select every proposed move' }));
    expect(screen.getByRole('button', { name: 'Apply selected (2)' })).toBeTruthy();
  });

  it('counts every pending decision and pages through them', async () => {
    const getDecisions = vi
      .spyOn(api, 'getDecisions')
      .mockResolvedValue(paginated([decision()], { total_pages: 3, total: 120 }));
    vi.spyOn(api, 'countDecisions').mockResolvedValue(0);
    renderWithI18n(Simulation);

    expect(await screen.findByText('Decisions awaiting review: 120')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));

    await waitFor(() =>
      expect(getDecisions).toHaveBeenLastCalledWith(
        expect.objectContaining({ status: 'pending', page: 2 }),
        expect.any(AbortSignal),
      ),
    );
  });

  it('offers the latest proposals once a newer simulation replaced the run', async () => {
    await show([]);
    const reviewed = simulation([decision()]);
    const latest = simulation([decision({ media_title: 'Totoro' })]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(reviewed);
    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await screen.findByText('Akira');

    vi.spyOn(api, 'getTasks').mockResolvedValue(
      paginated([job({ kind: 'simulate', result: latest })]),
    );
    RUNS.set(reviewed.simulation_id, []);
    await userEvent.selectOptions(filterField('Action'), 'move');

    expect(await screen.findByText('A newer simulation replaced these proposals.')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Show the latest proposals' }));

    expect(await screen.findByText('Totoro')).toBeTruthy();
    expect(api.getDecisions).toHaveBeenLastCalledWith(
      expect.objectContaining({ simulation_id: latest.simulation_id, action: 'move', page: 1 }),
      expect.any(AbortSignal),
    );
  });

  it('narrows the list and Apply all to the filters, and names the apply by them', async () => {
    vi.spyOn(api, 'getCategories').mockResolvedValue([category({ name: 'anime' })]);
    vi.spyOn(api, 'getRules').mockResolvedValue([rule()]);
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision(), decision()]));
    const applyAll = vi.spyOn(api, 'applyAllDecisions').mockResolvedValue(batchReport);
    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    expect(await screen.findByRole('button', { name: 'Apply all (2)' })).toBeTruthy();

    filterField('Rule');
    await screen.findByRole('option', { name: 'Japanese animation' });
    await userEvent.selectOptions(filterField('Category'), 'anime');
    await userEvent.selectOptions(filterField('Rule'), 'r1');

    await waitFor(() =>
      expect(api.getDecisions).toHaveBeenLastCalledWith(
        expect.objectContaining({ category: 'anime', rule_id: 'r1', page: 1 }),
        expect.any(AbortSignal),
      ),
    );
    await userEvent.click(
      await screen.findByRole('button', { name: 'Apply the matching moves (2)' }),
    );
    await waitFor(() => expect(applyAll).toHaveBeenCalledTimes(1));
    expect(nthCall(applyAll)[1]).toEqual({ category: 'anime', rule_id: 'r1' });

    await userEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(await screen.findByRole('button', { name: 'Apply all (2)' })).toBeTruthy();
  });

  it('applies nothing past a filter on skips, which keeps no move', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(
      simulation([decision(), decision({ action: 'skip' })]),
    );
    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await screen.findByRole('button', { name: 'Apply all (1)' });

    await userEvent.selectOptions(filterField('Action'), 'skip');

    await waitFor(() => expect(screen.queryByRole('button', { name: /Apply/ })).toBeNull());
    expect(screen.getAllByRole('row')).toHaveLength(2);
  });

  it('says the filters keep nothing rather than asking for a run', async () => {
    await show([]);
    await userEvent.type(screen.getByRole('searchbox', { name: 'Search a title' }), 'heat');

    expect(await screen.findByText('No decision matches these filters.')).toBeTruthy();
  });

  it('says the filter choices could not be read', async () => {
    vi.spyOn(api, 'getRules').mockRejectedValue(
      new ApiError('The rules are unreadable', 503, 'unavailable'),
    );
    await show([]);

    expect(await screen.findByText('The rules are unreadable')).toBeTruthy();
  });
});

/**
 * A reader is refused a run, an apply and a keep, and the settings the batch limit is read from.
 * The proposals waiting, their reasons and the filters are theirs to read.
 */
describe('for a reader', () => {
  const offered = () =>
    [/^Run simulation$/, /^Apply selected/, /^Keep here – /, /^Select /].filter(
      (name) =>
        [
          ...screen.queryAllByRole('button', { name }),
          ...screen.queryAllByRole('checkbox', { name }),
        ].length > 0,
    );

  it('offers the owner the run, the apply, the keep and the selection, and a reader none', async () => {
    const { unmount } = await show([decision({ media_title: 'Akira' })]);
    await screen.findByRole('button', { name: /^Apply selected/ });
    expect(offered()).toHaveLength(4);
    unmount();

    publishAccess(true);
    await show([decision({ media_title: 'Akira' })]);

    expect(await screen.findByText('Akira')).toBeInTheDocument();
    expect(screen.getByText('Decisions awaiting review: 1')).toBeInTheDocument();
    expect(offered()).toEqual([]);
    expect(screen.queryByLabelText(/Move the files on disk too/)).toBeNull();
    expect(screen.getByRole('button', { name: /^Filters/ })).toBeInTheDocument();
  });

  it('lists the proposals for a reader without asking for the settings', async () => {
    publishAccess(true);
    await show([decision({ media_title: 'Akira' })]);

    expect(await screen.findByText('Akira')).toBeInTheDocument();
    expect(api.getSettings).not.toHaveBeenCalled();
  });

  it('lists the proposals when the settings are refused, at the default batch limit', async () => {
    vi.mocked(api.getSettings).mockRejectedValue(
      new ApiError('You can read this Routarr and change nothing in it.', 403, 'forbidden'),
    );
    await show([decision({ media_title: 'Akira' })]);

    expect(await screen.findByText('Akira')).toBeInTheDocument();
    expect(screen.queryByText(/change nothing in it/)).toBeNull();
  });

  it('still says a failed read of the settings that is no refusal', async () => {
    vi.mocked(api.getSettings).mockRejectedValue(new ApiError('Locked.', 503, 'unavailable'));
    await show([decision({ media_title: 'Akira' })]);

    expect(await screen.findByText('Locked.')).toBeInTheDocument();
  });
});
