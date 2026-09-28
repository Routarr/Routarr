import { SECTIONS } from '../src/lib/settings';

/**
 * The screens the sweeps open: the shell's own route table, not a list kept by
 * hand beside it. Kept by hand, one list lacked `/rules/tests` and the console
 * sweep never opened it.
 */
export { SCREENS } from '../src/lib/routes';

/** Each section of the settings screen, which shows one at a time. */
export const SETTINGS_SECTIONS = SECTIONS.map(({ id }) => `/settings#${id}`);
