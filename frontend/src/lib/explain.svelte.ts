import { api } from '../api/client';
import type { Explanation } from '../api/types';
import { describeError } from './async.svelte';

/**
 * The explanation a screen opens from a title's "Why?", one at a time: a title asked about later
 * takes the place of one still on its way, whose late answer would open the wrong explanation.
 * A failed read is the screen's to show, apart from its outcome, since nothing was done.
 */
export function createExplainer() {
  const state = $state<{ asked: string | null; shown: Explanation | null; failure: string | null }>(
    { asked: null, shown: null, failure: null },
  );
  let asking: AbortController | null = null;

  return {
    /** The title whose explanation is on its way, which its button says. */
    get asked(): string | null {
      return state.asked;
    },
    get shown(): Explanation | null {
      return state.shown;
    },
    /** Why the last explanation could not be read, or null. */
    get failure(): string | null {
      return state.failure;
    },
    async open(mediaId: string): Promise<void> {
      asking?.abort();
      const mine = (asking = new AbortController());
      state.asked = mediaId;
      try {
        const answer = await api.explainTitle(mediaId, mine.signal);
        if (mine.signal.aborted) return;
        state.shown = answer;
        state.failure = null;
      } catch (err) {
        if (!mine.signal.aborted) state.failure = describeError(err);
      } finally {
        if (!mine.signal.aborted) state.asked = null;
      }
    },
    close(): void {
      state.shown = null;
    },
    dismiss(): void {
      state.failure = null;
    },
  };
}

export type Explainer = ReturnType<typeof createExplainer>;
