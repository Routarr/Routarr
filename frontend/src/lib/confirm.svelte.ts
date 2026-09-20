/**
 * The one confirmation dialog.
 *
 * `window.confirm()` costs three things here. Its buttons are browser chrome,
 * so they render in the *browser's* language however carefully the question is
 * translated — in an application whose language is a setting with twenty-six
 * catalogues behind it. It cannot be asserted on: jsdom ships no `confirm` at
 * all, so a unit test can only assert against a stand-in of its own making, and
 * a Playwright journey needs a blanket `page.on('dialog')` handler that accepts
 * whatever is asked. And a browser told to suppress further dialogs returns
 * `false` without asking, which on the apply guardrail means the action
 * silently does not happen.
 *
 * A module-level rune rather than a context, for the reason the dictionary is
 * one: there is exactly one dialog, and every caller wants the same one.
 * `Layout` mounts the component once; callers only await.
 */

/** One button. `label` is a dictionary key, not a rendered string. */
export type Choice = { label: string; value: string; danger?: boolean };

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
 * Ask, and resolve with the chosen value — or `null` for Cancel and Escape.
 *
 * Several choices are supported because two of these questions are not yes/no.
 * Squeezed into one, the rule import asks "replace everything?" and appends on
 * Cancel, so a user who presses Escape imports the file anyway with no way to
 * abort. `confirm()` cannot express three outcomes; this can, and Cancel means
 * cancel.
 */
export function ask(message: string, choices: Choice[]): Promise<string | null> {
  // A second question while one is open would strand the first promise for
  // good. Nothing opens two today, but an unresolved promise is a hang.
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
