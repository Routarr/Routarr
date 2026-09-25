import { describeError } from './async.svelte';

/**
 * What an action says about how it went: one success, one partial result or
 * one error, never two at once.
 *
 * A banner stays until something replaces it, so a page holding its success
 * and its error as separate pieces of state can show both, the previous
 * success above the reason the next action was refused, and the older, louder
 * sentence is the one read. Here each replaces the others, so no two can be on
 * screen together whatever an action forgets. A cancelled question reports
 * nothing and so leaves the last outcome standing.
 *
 * A partial result is its own state: part done and part failed is neither,
 * and red over forty-nine moves made would say nothing was done.
 *
 * The error is the action's own, apart from the error of the page's load. A
 * load can be retried and a reload replaces its error, while a refused write
 * is the operator's to try again: under the load's Retry, a refusal would
 * reload the page instead of replaying the action, and that reload would wipe
 * it. Render it through `OutcomeBanner`, which offers no Retry.
 */
export interface Outcome {
  readonly notice: string | null;
  readonly warning: string | null;
  readonly error: string | null;
  /** The items a partial or failed result is made of, shown under it and gone with it. */
  readonly details: readonly string[];
  /** Shows a success, and takes anything else off screen. */
  succeed(message: string): void;
  /** Shows a partial result, part done and part failed, and takes anything else off screen. */
  warn(message: string, details?: string[]): void;
  /** Shows an error, and takes anything else off screen. A string is shown as it is. */
  fail(cause: unknown, details?: string[]): void;
  /**
   * Takes the outcome off screen, for an action that succeeded with nothing to say:
   * an export that downloaded its file, a key that was regenerated. Without it
   * the error from the attempt before would still be standing over a success.
   */
  clear(): void;
}

export function createOutcome(): Outcome {
  let notice = $state<string | null>(null);
  let warning = $state<string | null>(null);
  let error = $state<string | null>(null);
  let details = $state<string[]>([]);
  const show = (next: {
    notice?: string;
    warning?: string;
    error?: string;
    details?: string[];
  }) => {
    notice = next.notice ?? null;
    warning = next.warning ?? null;
    error = next.error ?? null;
    details = next.details ?? [];
  };
  return {
    get notice() {
      return notice;
    },
    get warning() {
      return warning;
    },
    get error() {
      return error;
    },
    get details() {
      return details;
    },
    succeed(message) {
      show({ notice: message });
    },
    warn(message, items = []) {
      show({ warning: message, details: items });
    },
    fail(cause, items = []) {
      show({ error: describeError(cause), details: items });
    },
    clear() {
      show({});
    },
  };
}
