import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { decision, job, paginated } from '../test/fixtures';
import { ApiError, api, type Following } from '../api/client';
import type { Decision, SimulationResult } from '../api/types';
import Simulation from './Simulation.svelte';
import { answerConfirmation } from '../test/confirm';
import { dropFocus } from '../test/focus';
import { statusRevision } from '../lib/status.svelte';

/**
 * The screen that writes.
 *
 * Everything the executor guards against (applying nothing, applying past the
 * confirmation threshold, applying a proposal the user never looked at) is
 * enforced again here, in the browser, before a request is made at all. The
 * end-to-end journeys drive the happy path, which is the one case where a
 * missing guard cannot be seen.
 */

const STRINGS = {
  Simulation: 'Simulation',
  RunSimulation: 'Run simulation',
  EvaluatingRules: 'Evaluating…',
  ApplySelected: 'Apply selected ({count})',
  ApplyAll: 'Apply all ({count})',
  SelectMoveFor: 'Select the move for "{title}"',
  SelectEveryMove: 'Select every move',
  ConfirmApplyItems: '{count} items will move.',
  ConfirmApplyItemsWithFiles: '{count} items will move, files too.',
  SimulationEmptyState: 'Nothing to review',
  ApplyResult: 'Applied: {applied} of {requested}.',
  ApplyResultWithSkipped: 'Applied: {applied} of {requested}, stale: {skipped}.',
  BatchApplyReport: 'Applied in batches: {applied} of {candidates}',
  BatchApplyStopped: 'Stopped at batch {run} of {planned}: {applied} of {candidates} applied',
  Dismiss: 'Dismiss',
  MoveFilesLabel: 'Move the files on disk too',
  ApplyStillMoving: 'Still moving: {count}',
  ApplyReplaced: 'Replaced: {count}',
  ApplyStoppedOnRequest: 'Stopped on request',
  StopTask: 'Stop',
  ApplyAllHint: 'Every move this run proposed, in batches.',
};

/** What each run stored, as `/decisions` lists it under the run's id. */
const RUNS = new Map<string, Decision[]>();

/**
 * A stored run's report, which keeps its counts alone, its proposals left
 * for `/decisions` to list.
 */
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

/** Render and wait for the pending-decisions fetch the page makes on mount. */
async function show(pending: Decision[]) {
  vi.spyOn(api, 'getDecisions').mockImplementation(async (params) =>
    paginated(
      typeof params?.simulation_id === 'string' ? (RUNS.get(params.simulation_id) ?? []) : pending,
    ),
  );
  renderWithI18n(Simulation, { strings: STRINGS });
  await screen.findByRole('heading', { name: 'Simulation' });
}

// No run is going on when the screen opens, unless a test says otherwise.
beforeEach(() => vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([])));

afterEach(() => vi.restoreAllMocks());

describe('what the screen shows', () => {
  /**
   * The dashboard sends the reader here with a count of decisions to review.
   * The empty state ("run a simulation") would contradict it without a word
   * about why, so a failed request renders a banner.
   */
  it('says the pending list could not be loaded rather than pretending it is empty', async () => {
    vi.spyOn(api, 'getDecisions').mockRejectedValue(new ApiError('later', 503, 'unavailable'));
    renderWithI18n(Simulation, { strings: STRINGS });

    expect(await screen.findByRole('alert')).toHaveTextContent('later');
  });

  /** Loaded as every screen loads, so a failed list is one click from a retry. */
  it('asks the pending list again when the reader retries', async () => {
    const pendingDecision = decision({ id: 'd-retry', media_title: 'Retried' });
    vi.spyOn(api, 'getDecisions')
      .mockRejectedValueOnce(new ApiError('later', 503, 'unavailable'))
      .mockResolvedValue(paginated([pendingDecision]));
    renderWithI18n(Simulation, { strings: { ...STRINGS, Retry: 'Retry' } });

    await fireEvent.click(await screen.findByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('Retried')).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  /** "Nothing to review" before the list has answered is a claim nobody checked. */
  it('holds the table open while the pending list loads', async () => {
    vi.spyOn(api, 'getDecisions').mockReturnValue(new Promise(() => {}));
    renderWithI18n(Simulation, { strings: { ...STRINGS, Loading: 'Loading' } });

    expect(await screen.findByText('Loading')).toHaveAttribute('role', 'status');
    expect(screen.getByRole('table', { name: 'Simulation' })).toBeTruthy();
    expect(screen.queryByText('Nothing to review')).toBeNull();
  });

  /**
   * Decisions a previous pass left are persisted, so the screen shows them, not
   * only what this browser session ran: a count of them on the dashboard would
   * otherwise land on an empty state telling the reader to run a simulation.
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

  /** The shell draws the guide, whose simulation step hears of a run only through this. */
  it('tells the shell a simulation ran', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    const before = statusRevision();

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /**
   * A run pre-selects what it proposes so the common case is one click. It must
   * pre-select only what is actionable: a skip has no target folder, and
   * checking it would send the executor a decision it can only refuse.
   */
  it('pre-selects the moves and leaves the skips alone', async () => {
    await show([]);
    const moving = decision({ media_title: 'Akira' });
    const skip = decision({ media_title: 'Dune', action: 'skip', target_root_folder: null });
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([moving, skip]));
    const apply = vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await waitFor(() => expect(screen.getByText('Akira')).toBeTruthy());

    // One move proposed, so one row is checkable and it is checked.
    const boxes = screen.getAllByRole('checkbox', { name: /select the move for/i });
    expect(boxes).toHaveLength(1);
    expect((boxes[0] as HTMLInputElement).checked).toBe(true);

    // And the ids that leave for the executor are that one alone. Asserted on
    // the request rather than on the checkboxes: a skip renders none, so a
    // pre-selection that wrongly included it would be invisible on screen and
    // arrive at the server all the same.
    await fireEvent.click(screen.getByRole('button', { name: /apply selected/i }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(nthCall(apply)[0]).toEqual([moving.id]);
  });
});

/**
 * Whether the Arr moves the files on disk is the one choice an apply cannot
 * take back, and it travels as a flag no other part of the screen shows.
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
    await fireEvent.click(await screen.findByLabelText('Move the files on disk too'));
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
    expect(nthCall(applyAll)[1]).toBe(false);

    await fireEvent.click(await screen.findByLabelText('Move the files on disk too'));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));
    await waitFor(() => expect(applyAll).toHaveBeenCalledTimes(2));
    expect(nthCall(applyAll, 1)[1]).toBe(true);
  });
});

describe('what the screen refuses to do', () => {
  it('applies nothing when nothing is selected', async () => {
    const apply = vi.spyOn(api, 'applyDecisions');
    // Pending decisions are shown but not pre-selected: only a run selects.
    await show([decision({ media_title: 'Akira' })]);

    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    // Settled, then asserted: a `waitFor` around a negative passes on its
    // first look, before the handler has had a turn to make the call.
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
    expect(nthCall(applyAll)[2]).toEqual([]);
  });

  /**
   * Past the configured threshold the backend refuses and asks for an explicit
   * second pass. That is a question, not a failure: showing it as a red banner
   * would tell the user their moves had gone wrong when nothing had happened.
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

    // The refusal's own message is carried into the question rather than
    // discarded: it is the only place the threshold is named.
    expect(await answerConfirmation()).toContain('Above the threshold');

    // Asked once, then applied again naming the guardrail that was answered,
    // and only that one, so a second refusal still gets its own question.
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(nthCall(apply)[2]).toEqual([]);
    expect(nthCall(apply, 1)[2]).toEqual(['threshold']);
  });

  /**
   * The batch question is the server's, because only the server knows that a
   * destination is not answering or short of room. Answered in advance, those
   * facts never reach the reader, and a sleeping NAS gets the batches after
   * one generic yes.
   */
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
    expect(nthCall(applyAll)[2]).toEqual([]);
    expect(nthCall(applyAll, 1)[2]).toEqual(['batch', 'unreachable']);
  });

  /** What Apply all reaches is said beside it, not in a title only a mouse shows. */
  it('says that Apply all reaches past the rows on screen', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    const button = await screen.findByRole('button', { name: /apply all/i });
    expect(button).toHaveAccessibleDescription('Every move this run proposed, in batches.');
    expect(screen.getByText('Every move this run proposed, in batches.')).toBeInTheDocument();
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
    expect(screen.getByRole('button', { name: 'Applying' })).toBeDisabled();
    expect(screen.getByRole('button', { name: /apply all/i })).toBeDisabled();
  });
});

/**
 * One outcome of applying on screen, the latest. A report stays until something
 * replaces it, and this is the screen that moves files: a report of moves that
 * worked, left above the reason the next apply was refused, reads as the result
 * of that apply.
 */
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
    expect(await screen.findByText('Applied: 1 of 1.')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    expect(await screen.findByText('The Arr refused the move')).toBeTruthy();
    expect(screen.queryByText('Applied: 1 of 1.')).toBeNull();
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
    expect(await screen.findByText('Applied in batches: 1 of 1')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    expect(await screen.findByText('Applied: 1 of 1.')).toBeTruthy();
    expect(screen.queryByText('Applied in batches: 1 of 1')).toBeNull();
  });

  /** An apply in which every decision failed is a failure, not a count of zero. */
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

    const summary = await screen.findByText('Applied: 0 of 1.');
    expect(summary.closest('[role]')?.getAttribute('role')).toBe('alert');
    expect(screen.getByText('Akira: The Arr refused the move')).toBeTruthy();
  });

  /**
   * The moves were made even when the refresh after them fails, so the
   * refresh's failure is shown beside the report and never in its place.
   */
  it('keeps the report of an apply whose refresh failed', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation')
      .mockResolvedValueOnce(simulation([decision({ media_title: 'Akira' })]))
      .mockRejectedValueOnce(new ApiError('The library could not be read', 409, 'conflict'));
    vi.spyOn(api, 'applyDecisions').mockResolvedValue(applied);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    expect(await screen.findByText('The library could not be read')).toBeTruthy();
    expect(screen.getByText('Applied: 1 of 1.')).toBeTruthy();
  });

  /** The apply moved what the shell counts, whatever the refresh after it ends in. */
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
   * Part done and part failed is neither a success nor a failure. Red over
   * forty-nine moves made would say nothing was done, green would hide the
   * one that was not.
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
        stopped: null,
        errors: [{ decision_id: 'd1', media_title: 'Akira', message: 'The Arr refused the move' }],
      });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));
    await answerConfirmation();

    const summary = await screen.findByText('Stopped at batch 1 of 1: 49 of 50 applied');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
  });

  /**
   * A move the Arr is still making and a proposal replaced since leave the
   * apply unfinished, each said beside the count.
   */
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

    const summary = await screen.findByText('Applied: 1 of 3.');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText('Still moving: 1')).toBeTruthy();
    expect(screen.getByText('Replaced: 1')).toBeTruthy();
  });

  /** Stop reaches the task the screen follows, and the report says why it ended. */
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
    const cancelJob = vi.spyOn(api, 'cancelJob').mockResolvedValue(undefined);

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    finish();

    expect(cancelJob).toHaveBeenCalledWith('j-apply');
    expect(await screen.findByText('Stopped on request')).toBeTruthy();
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

    expect(await screen.findByText('Applied: 1 of 2, stale: 1.')).toBeTruthy();
  });

  it('asks about the files in the same sentence as the count', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));
    vi.spyOn(api, 'applyDecisions').mockRejectedValue(batchQuestion());

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByLabelText('Move the files on disk too'));
    await fireEvent.click(screen.getByRole('button', { name: /apply selected/i }));

    expect(await answerConfirmation(null)).toContain('1 items will move, files too.');
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

    const summary = await screen.findByText('Applied: 1 of 2.');
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
    expect(await screen.findByText('Applied: 1 of 1.')).toBeTruthy();
    const rerun = await screen.findByRole('button', { name: /run simulation/i });
    await waitFor(() => expect((rerun as HTMLButtonElement).disabled).toBe(false));

    await fireEvent.click(rerun);

    await waitFor(() => expect(screen.queryByText('Applied: 1 of 1.')).toBeNull());
  });

  /** The moves that failed belong to the outcome they explain, and go with it. */
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

  /**
   * An apply button turns disabled while it writes, which drops its focus to
   * the start of the page, where the next Tab begins again from the top.
   */
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

/**
 * A run of any size is followed through its task rather than waited for, so
 * no request bound cuts it, and the screen shows it going on.
 */
describe('a run followed through its task', () => {
  it('shows how far the run has gone while it is followed', async () => {
    await show([]);
    vi.spyOn(api, 'runSimulation').mockImplementation((_, following?: Following) => {
      following?.onProgress?.(job({ progress_current: 50, progress_total: 200 }));
      return new Promise(() => {});
    });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));

    const bar = await screen.findByRole('progressbar', { name: 'Evaluating…' });
    expect(bar).toHaveAttribute('aria-valuenow', '50');
    expect(bar).toHaveAttribute('aria-valuemax', '200');
  });

  /** Left and opened again, the screen finds the run still going and waits for it. */
  it('follows a run already going when the screen opens, and lists what it proposed', async () => {
    vi.spyOn(api, 'getJobs').mockResolvedValue(paginated([job({ id: 'j9', kind: 'simulate' })]));
    let finish: (result: SimulationResult) => void = () => {};
    const follow = vi
      .spyOn(api, 'followJob')
      .mockReturnValue(new Promise((resolve) => (finish = resolve as never)));
    await show([]);

    await waitFor(() => expect(follow).toHaveBeenCalledWith('j9', expect.anything()));
    expect(screen.getByRole('button', { name: 'Evaluating…' })).toBeDisabled();

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
    const view = renderWithI18n(Simulation, { strings: STRINGS });
    await fireEvent.click(await screen.findByRole('button', { name: /run simulation/i }));
    expect(signal?.aborted).toBe(false);

    view.unmount();

    expect(signal?.aborted).toBe(true);
  });

  /** Listed by the run's id, a page at a time, however many it proposed. */
  it('pages through what the run proposed', async () => {
    const getDecisions = vi
      .spyOn(api, 'getDecisions')
      .mockResolvedValue(paginated([decision()], { total_pages: 2, total: 250 }));
    renderWithI18n(Simulation, {
      strings: { ...STRINGS, Next: 'Next', PageOf: 'Page {page} of {total}' },
    });
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
