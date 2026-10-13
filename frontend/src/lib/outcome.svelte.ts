import { describeError } from './async.svelte';

/**
 * What an action says about how it went: one success, one partial result or one error, each
 * replacing the others. A cancelled question reports nothing and leaves the last one standing.
 *
 * The error stays apart from the page's load error, whose Retry would reload the page instead
 * of replaying a refused write. Render it through `OutcomeBanner`, which offers no Retry.
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
  /** Takes the outcome off screen, for a success with nothing to say, as a download. */
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
