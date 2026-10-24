import { render as testingLibraryRender } from '@testing-library/svelte';
import { seedDictionary } from '../lib/i18n.svelte';
import { SERVER_COUNTS } from './counts';
import { readBackend } from './backend';

type Render = typeof testingLibraryRender;

/** The English dictionary the server serves, every other language merged over it. */
export const ENGLISH: Readonly<Record<string, string>> = JSON.parse(
  readBackend('backend/locales/en.json'),
) as Record<string, string>;

/**
 * Render with the dictionary the server would serve, seeded before the first pass since `t()`
 * reads it then. Every key of `backend/locales/en.json` is seeded, and `strings` merge over
 * it, as the server merges a language over English.
 */
export function renderWithI18n(
  component: Parameters<Render>[0],
  options: {
    props?: Record<string, unknown>;
    strings?: Record<string, string>;
    /** The interface language, English unless a test reads another one. */
    language?: string;
    /** As the server would answer for that language: left to right unless a test says so. */
    direction?: 'ltr' | 'rtl';
  } = {},
) {
  seedDictionary(
    { ...ENGLISH, ...options.strings },
    options.language,
    SERVER_COUNTS,
    options.direction,
  );
  return testingLibraryRender(component, options.props as never);
}
