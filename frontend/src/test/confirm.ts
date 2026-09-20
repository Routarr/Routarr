import { expect } from 'vitest';
import { waitFor } from '@testing-library/svelte';

import { confirmation, settle } from '../lib/confirm.svelte';

/**
 * Answer the pending confirmation, and hand back what it asked.
 *
 * Installing a `window.confirm` — jsdom ships none — and asserting it was
 * called is a stub answering a stub, and cannot tell the right question from
 * any question. Waiting on the real request means a test reads the sentence the
 * user would have read.
 *
 * `value` is the button pressed; `null` is Cancel and Escape.
 */
export async function answerConfirmation(value: string | null = 'confirm'): Promise<string> {
  await waitFor(() => expect(confirmation.request).not.toBeNull());
  const asked = confirmation.request?.message ?? '';
  settle(value);
  return asked;
}
