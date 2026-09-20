/**
 * When the shell's copy of `/status` has to be read again.
 *
 * The top bar and the navigation share one `createAsync` in `Layout`, so they
 * never disagree with each other — they disagree with the *page*. A screen that
 * maps a category, enables an instance or saves a metadata key has just removed
 * a warning the bar is still counting, and nothing told the shell. Until this,
 * the only thing that eventually corrected it was the idle poll, a minute away.
 *
 * A revision rather than a second copy of the state: `Layout` already owns the
 * request and its loading, error and 401 handling, and a store beside it would
 * be a second source to keep in step. Bumping this re-runs that one loader
 * through its `deps`, which drops a late response through the generation
 * counter it already has — one request, no duplicate, no global refresh.
 *
 * A module-level rune for the reason the dictionary and the confirmation dialog
 * are: there is exactly one shell, and every caller means the same one.
 */
const state = $state({ revision: 0 });

/**
 * Say that something the warnings are computed from has changed.
 *
 * Call it after a write that could add or remove one, never on a read. The
 * warnings are derived server-side from the instances, the mappings, the
 * metadata sources and the jobs, so those are the writes that matter.
 */
export function invalidateStatus(): void {
  state.revision += 1;
}

/** Read inside `deps` so the shell's loader re-runs when this moves. */
export function statusRevision(): number {
  return state.revision;
}
