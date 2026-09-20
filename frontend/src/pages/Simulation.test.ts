import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { decision, paginated } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import type { Decision, SimulationResult } from '../api/types';
import Simulation from './Simulation.svelte';
import { answerConfirmation } from '../test/confirm';

/**
 * The screen that writes.
 *
 * Everything the executor guards against — applying nothing, applying past the
 * confirmation threshold, applying a proposal the user never looked at — is
 * enforced again here, in the browser, before a request is made at all. The
 * end-to-end journeys drive the happy path, which is the one case where a
 * missing guard cannot be seen.
 */

const STRINGS = {
  SimulationTitle: 'Simulation',
  RunSimulation: 'Run simulation',
  EvaluatingRules: 'Evaluating…',
  ApplySelected: 'Apply selected ({count})',
  ApplyAll: 'Apply all ({count})',
  SelectMoveFor: 'Select the move for “{title}”',
  SelectEveryMove: 'Select every move',
  ConfirmApply: '{count} items will move.',
  ConfirmApplyAll: '{count} items will move.',
  ConfirmApplyWithFiles: ' Files move too.',
  SimulationEmptyState: 'Nothing to review',
};

function simulation(decisions: Decision[]): SimulationResult {
  return {
    simulation_id: 's2',
    capacity: [],
    returned: decisions.length,
    decisions,
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

/** Render and wait for the pending-decisions fetch the page makes on mount. */
async function show(pending: Decision[]) {
  vi.spyOn(api, 'getDecisions').mockResolvedValue(paginated(pending));
  renderWithI18n(Simulation, { strings: STRINGS });
  await screen.findByRole('heading', { name: 'Simulation' });
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('what the screen shows', () => {
  /**
   * The navigation sends the user here with "12 decisions to review". A failed
   * request answered with the empty state — "run a simulation" — contradicted
   * it without a word about why; a failed request renders a banner.
   */
  it('says the pending list could not be loaded rather than pretending it is empty', async () => {
    vi.spyOn(api, 'getDecisions').mockRejectedValue(new ApiError('later', 503, 'unavailable'));
    renderWithI18n(Simulation, { strings: STRINGS });

    expect(await screen.findByRole('alert')).toHaveTextContent('later');
  });

  /**
   * The top bar counts pending decisions and links here. Showing only what the
   * current browser session ran lands "12 decisions awaiting review" on an empty
   * state telling the user to run a simulation — for twelve decisions that are
   * already persisted.
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
    const apply = vi
      .spyOn(api, 'applyDecisions')
      .mockResolvedValue({ requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] });

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

  /** The a11y fix, pinned where it is cheap: the name says which film. */
  it('names each row checkbox after its film', async () => {
    await show([decision({ media_title: 'Perfect Blue' })]);

    expect(
      await screen.findByRole('checkbox', { name: 'Select the move for “Perfect Blue”' }),
    ).toBeTruthy();
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
    const applyAll = vi.spyOn(api, 'applyAllDecisions');
    await show([]);
    vi.spyOn(api, 'runSimulation').mockResolvedValue(simulation([decision()]));

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply all/i }));

    // The question says how many, so a user who miscounted stops here.
    expect(await answerConfirmation(null)).toMatch(/1/);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(applyAll).not.toHaveBeenCalled();
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
      .mockResolvedValue({ requested: 1, applied: 1, failed: 0, skipped: 0, errors: [] });

    await fireEvent.click(screen.getByRole('button', { name: /run simulation/i }));
    await fireEvent.click(await screen.findByRole('button', { name: /apply selected/i }));

    // The refusal's own message is carried into the question rather than
    // discarded — it is the only place the threshold is named.
    expect(await answerConfirmation()).toContain('Above the threshold');

    // Asked once, then applied again naming the guardrail that was answered —
    // and only that one, so a second refusal still gets its own question.
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(nthCall(apply)[2]).toEqual([]);
    expect(nthCall(apply, 1)[2]).toEqual(['threshold']);
  });
});
