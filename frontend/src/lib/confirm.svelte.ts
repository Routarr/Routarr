import { ApiError } from '../api/client';

/**
 * The one confirmation dialog, in place of `window.confirm()`, whose buttons speak the browser's
 * language, which no test can answer, and which a browser told to suppress dialogs answers
 * `false` unasked. A module-level rune: `Layout` mounts the dialog once, and callers await.
 */

/** One button. `label` is a dictionary key, not a rendered string. */
type Choice = { label: string; value: string; danger?: boolean };

type Request = {
  message: string;
  choices: Choice[];
  resolve: (value: string | null) => void;
};

const state = $state<{ request: Request | null }>({ request: null });

export const confirmation = {
  get request(): Request | null {
    return state.request;
  },
};

/**
 * Ask, and resolve with the chosen value, or `null` for Cancel and Escape. Several choices,
 * since a question such as the rule import's has three outcomes and Cancel must mean cancel.
 */
export function ask(message: string, choices: Choice[]): Promise<string | null> {
  // A second question while one is open would strand the first promise for
  // good. No caller opens two, but an unresolved promise is a hang.
  state.request?.resolve(null);
  return new Promise((resolve) => {
    state.request = { message, choices, resolve };
  });
}

/** The yes/no shape: true only when the primary button was pressed. */
export function askConfirmation(message: string, label: string, danger = true): Promise<boolean> {
  return ask(message, [{ label, value: 'confirm', danger }]).then((v) => v === 'confirm');
}

/** Close with an answer. `null` is Cancel, Escape, and the backdrop. */
export function settle(value: string | null): void {
  const pending = state.request;
  state.request = null;
  pending?.resolve(value);
}

/**
 * Send a write, and ask each question the backend raises, in its words followed by `after`,
 * under a button reading `label`. Each guardrail is answered by name, never by a blanket yes,
 * and the list grows only by what a question asked or stated. `null` is a question declined,
 * and nothing was written.
 */
export async function answering<T>(
  send: (answered: string[]) => Promise<T>,
  label: string,
  after = '',
  answered: string[] = [],
): Promise<T | null> {
  try {
    return await send(answered);
  } catch (err) {
    if (!(err instanceof ApiError && err.needsConfirmation && err.confirm)) throw err;
    const asking = err.confirm;
    const question = after ? `${err.message}\n\n${after}` : err.message;
    if (!(await askConfirmation(question, label))) return null;
    return answering(send, label, after, [...answered, asking, ...err.includes]);
  }
}
