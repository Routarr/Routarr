import { SECTIONS } from '../src/lib/settings';

/** The screens the sweeps open, read from the shell's own route table. */
export { SCREENS } from '../src/lib/routes';

/**
 * The sections of the settings screen, which shows one at a time. A section on a
 * screen of its own, as the metadata sources, is in `SCREENS` already.
 */
export const SETTINGS_SECTIONS = SECTIONS.filter(({ page }) => page === 'settings').map(
  ({ id }) => `/settings#${id}`,
);
