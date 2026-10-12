import { expect } from 'vitest';
import { waitFor } from '@testing-library/svelte';

import { confirmation, settle } from '../lib/confirm.svelte';

/**
 * Answers the pending confirmation and returns the message it asked.
 *
 * `value` is the button pressed, and `null` is Cancel or Escape. Returns once a macrotask has
 * run after the answer: the component resumes on a later turn, and an assertion made before
 * then that nothing was sent passes against a component that ignores the answer.
 */
export async function answerConfirmation(value: string | null = 'confirm'): Promise<string> {
  await waitFor(() => expect(confirmation.request).not.toBeNull());
  const asked = confirmation.request?.message ?? '';
  settle(value);
  await new Promise((resolve) => setTimeout(resolve, 0));
  return asked;
}
