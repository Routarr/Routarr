import { describe, it, expect } from 'vitest';

import { ApiError } from '../api/client';
import { createOutcome } from './outcome.svelte';

/**
 * What an action says about how it went, kept apart from what the page's load
 * says: a reload can neither erase a refusal nor offer to retry a write.
 */
describe('createOutcome', () => {
  it('takes the error off screen when a success is shown', () => {
    const outcome = createOutcome();
    outcome.fail(new ApiError('The key was refused', 400, 'bad_request'));

    outcome.succeed('Settings saved');

    expect(outcome.notice).toBe('Settings saved');
    expect(outcome.error).toBeNull();
  });

  it('takes the success off screen when an error is shown', () => {
    const outcome = createOutcome();
    outcome.succeed('Rule duplicated');

    outcome.fail(new ApiError('The rule is pinned by a test case', 409, 'conflict'));

    expect(outcome.notice).toBeNull();
    expect(outcome.error).toBe('The rule is pinned by a test case');
  });

  /** A component that already worded its failure hands over the sentence. */
  it('shows a string it is given as it is', () => {
    const outcome = createOutcome();

    outcome.fail('Not a valid JSON file');

    expect(outcome.error).toBe('Not a valid JSON file');
  });

  /** An export retried after a failure downloads its file and says nothing. */
  it('takes both off screen for a success with nothing to say', () => {
    const outcome = createOutcome();
    outcome.fail('The export failed');

    outcome.clear();

    expect(outcome.notice).toBeNull();
    expect(outcome.error).toBeNull();
  });

  /** Part done and part failed is its own state, and replaces the others. */
  it('shows a partial result alone', () => {
    const outcome = createOutcome();
    outcome.fail('The Arr refused the move');

    outcome.warn('Moves made: 49 of 50');

    expect(outcome.warning).toBe('Moves made: 49 of 50');
    expect(outcome.error).toBeNull();
    expect(outcome.notice).toBeNull();
    outcome.succeed('Saved');
    expect(outcome.warning).toBeNull();
  });

  /** The failures a result is made of leave with it, whatever replaces it. */
  it('takes the details of a result off screen with it', () => {
    const outcome = createOutcome();
    outcome.warn('Moves made: 1 of 2', ['Heat: The Arr refused the move']);
    expect(outcome.details).toEqual(['Heat: The Arr refused the move']);

    outcome.succeed('Saved');

    expect(outcome.details).toEqual([]);
  });
});
