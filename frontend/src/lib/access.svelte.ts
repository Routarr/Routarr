/**
 * Whether the person signed in reads alone, as `/auth/me` said it to the shell.
 *
 * The server refuses a reader every write whatever this says: it only spares a reader the
 * controls that would be refused. Null until the shell has read it.
 */
const state = $state<{ readOnly: boolean | null }>({ readOnly: null });

export const access = {
  /** Whether `/auth/me` has answered, so a loader that differs for a reader can start. */
  get known(): boolean {
    return state.readOnly !== null;
  },
  /** A reader, refused every write. False until known. */
  get readOnly(): boolean {
    return state.readOnly === true;
  },
};

/** Called by the shell with what `/auth/me` answered, and with null to forget it. */
export function publishAccess(readOnly: boolean | null): void {
  state.readOnly = readOnly;
}
