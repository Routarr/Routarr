import { t } from './i18n.svelte';

/**
 * The JSON a file the reader picked holds. A file that is not JSON, a backup
 * archive picked by mistake, is said in the reader's language, not in the
 * parser's English.
 */
export async function readJsonFile(file: File): Promise<unknown> {
  const text = await file.text();
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new Error(t('NotValidJson'));
  }
}
