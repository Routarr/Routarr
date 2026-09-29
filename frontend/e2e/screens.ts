import { SECTIONS } from '../src/lib/settings';

/**
 * The screens the sweeps open: the shell's own route table, not a list kept by
 * hand beside it, so a screen added to the shell is swept without a line here.
 */
export { SCREENS } from '../src/lib/routes';

/** Each section of the settings screen, which shows one at a time. */
export const SETTINGS_SECTIONS = SECTIONS.map(({ id }) => `/settings#${id}`);
