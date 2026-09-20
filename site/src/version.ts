/**
 * Routarr's version, taken from the crate when the site is built.
 *
 * `backend/Cargo.toml` is the value the release tag names. Writing it here as
 * well would be a second copy nothing keeps in step until `check.mjs` runs, and
 * only when the site job does. Imported as text so it is inlined at bundle time:
 * a path read at prerender resolves under `dist/`, where the crate is not.
 */
import manifest from '../../backend/Cargo.toml?raw';

const match = manifest.match(/^version = "([^"]+)"/m);
if (!match?.[1]) throw new Error('backend/Cargo.toml carries no version');

export const version: string = match[1];
