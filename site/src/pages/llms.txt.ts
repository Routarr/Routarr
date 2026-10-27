/**
 * `/llms.txt` for language models (llmstxt.org), from `src/llms.txt` with `{tag}` read as the
 * release this build describes: the documents it links describe the version the pages name.
 */
import text from '../llms.txt?raw';
import { version } from '../version';

export function GET(): Response {
  return new Response(text.replaceAll('{tag}', `v${version}`));
}
