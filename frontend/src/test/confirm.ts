import { expect } from 'vitest';
import { waitFor } from '@testing-library/svelte';

import { confirmation, settle } from '../lib/confirm.svelte';

/**
 * Answer the pending confirmation, and hand back what it asked.
 *
 * Replacing `window.confirm` (jsdom's asks nothing and answers nothing) and
 * asserting it was called is a stub answering a stub, and cannot tell the
 * right question from any question. Waiting on the real request means a test reads the sentence the
 * user would have read.
 *
 * `value` is the button pressed, and `null` is Cancel and Escape.
 *
 * Returns once a macrotask has run after the answer: the component resumes on
 * a later turn, and an assertion that nothing was sent made before then passes
 * against a component that ignores the answer.
 */
export async function answerConfirmation(value: string | null = 'confirm'): Promise<string> {
  await waitFor(() => expect(confirmation.request).not.toBeNull());
  const asked = confirmation.request?.message ?? '';
  settle(value);
  await new Promise((resolve) => setTimeout(resolve, 0));
  return asked;
}
