import { describe, it, expect, vi } from 'vitest';

import { ApiError } from '../api/client';
import { answerConfirmation } from '../test/confirm';
import { answering } from './confirm.svelte';

const question = (name: string, includes: string[] = []) =>
  new ApiError(`Asked about ${name}.`, 409, 'confirmation_required', null, name, includes);

/**
 * A write two guardrails refuse asks twice, and the second request carries
 * both answers: one dropped, the server asks the first question again forever.
 */
describe('answering', () => {
  it('sends every answer given so far with each new request', async () => {
    const send = vi
      .fn<(answered: string[]) => Promise<string>>()
      .mockRejectedValueOnce(question('batch'))
      .mockRejectedValueOnce(question('capacity', ['unreachable']))
      .mockResolvedValue('written');

    const result = answering(send, 'Apply');
    expect(await answerConfirmation()).toBe('Asked about batch.');
    expect(await answerConfirmation()).toBe('Asked about capacity.');

    expect(await result).toBe('written');
    expect(send.mock.calls.map(([answered]) => answered)).toEqual([
      [],
      ['batch'],
      ['batch', 'capacity', 'unreachable'],
    ]);
  });

  it('writes nothing more once the second question is declined', async () => {
    const send = vi
      .fn<(answered: string[]) => Promise<string>>()
      .mockRejectedValueOnce(question('batch'))
      .mockRejectedValueOnce(question('capacity'))
      .mockResolvedValue('written');

    const result = answering(send, 'Apply');
    await answerConfirmation();
    await answerConfirmation(null);

    expect(await result).toBeNull();
    expect(send).toHaveBeenCalledTimes(2);
  });
});
