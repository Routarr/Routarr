import { render as testingLibraryRender } from '@testing-library/svelte';
import { seedDictionary } from '../lib/i18n.svelte';

type Render = typeof testingLibraryRender;

/**
 * Render with a seeded dictionary.
 *
 * The dictionary is a module-level rune, so seeding is a call rather than a
 * wrapper around every component under test — but it has to happen *before* the
 * component renders, since `t()` is read synchronously during the first pass.
 *
 * Only the strings a test asserts on are seeded. Anything absent renders as its
 * own key, which is exactly the production fallback.
 */
export function renderWithI18n(
  component: Parameters<Render>[0],
  options: { props?: Record<string, unknown>; strings?: Record<string, string> } = {},
) {
  seedDictionary(options.strings ?? {});
  return testingLibraryRender(component, options.props as never);
}
