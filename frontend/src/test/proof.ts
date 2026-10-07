import { expect } from 'vitest';
import { waitFor } from '@testing-library/svelte';

import { proofRequest, settleProof } from '../lib/proof.svelte';

/**
 * Answer the pending proof, and hand back what it asked for and why it asked
 * again. `value` is what was typed, and `null` is Cancel and Escape.
 *
 * Returns once a macrotask has run after the answer, as `answerConfirmation`
 * does: the caller resumes on a later turn.
 */
export async function answerProof(
  value: string | null,
): Promise<{ asked: string; note: string | undefined }> {
  await waitFor(() => expect(proofRequest.request).not.toBeNull());
  const asked = proofRequest.request?.asked ?? '';
  const note = proofRequest.request?.note;
  settleProof(value);
  await new Promise((resolve) => setTimeout(resolve, 0));
  return { asked, note };
}
