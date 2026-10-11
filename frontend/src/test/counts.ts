import { readBackend } from './backend';

/**
 * The count placeholders the server lists in `COUNTS` and sends with its
 * dictionary, read from its source so a test renders what the server would
 * have the interface group.
 */
export const SERVER_COUNTS: readonly string[] = (() => {
  const source = readBackend('backend/src/localization.rs');
  const list = /pub const COUNTS: &\[&str\] = &\[([^\]]*)\]/.exec(source)?.[1] ?? '';
  return [...list.matchAll(/"(\w+)"/g)].map((match) => match[1] ?? '');
})();
