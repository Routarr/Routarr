import { describe, it, expect } from 'vitest';

import { readJsonFile } from './upload';
import { seedDictionary } from './i18n.svelte';

describe('readJsonFile', () => {
  it('reads the JSON a file holds', async () => {
    const file = new File(['{"version":1}'], 'routarr-config.json');

    expect(await readJsonFile(file)).toEqual({ version: 1 });
  });

  it("refuses a file that is not JSON in the reader's words", async () => {
    seedDictionary({ NotValidJson: 'This file is not JSON.' });
    const file = new File(['PK\u0003\u0004'], 'routarr-backup.zip');

    await expect(readJsonFile(file)).rejects.toThrow('This file is not JSON.');
  });
});
