/**
 * When the shell's copy of `/status` has to be read again.
 *
 * `Layout` owns the one request, and a new revision re-runs it through its `deps`. Without
 * it, a warning a screen just removed stays counted until the next idle poll.
 */
const state = $state({ revision: 0 });

/**
 * Says that something the shell draws has changed.
 *
 * Call it after a write that can add or remove a warning (instances, mappings, metadata
 * sources, jobs) or move a step of the guide (rules, simulations), never after a read.
 */
export function invalidateStatus(): void {
  state.revision += 1;
}

/** Read inside `deps` so the shell's loader re-runs when this moves. */
export function statusRevision(): number {
  return state.revision;
}
