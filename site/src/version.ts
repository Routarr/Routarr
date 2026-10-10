/**
 * Routarr's version, as the page states it: the one `backend/Cargo.toml`
 * declares in this checkout. Production is built from a release's tag
 * (`.github/workflows/site.yml`), where the crate names that release.
 *
 * The crate is imported as text so it is inlined at bundle time: a path read
 * at prerender resolves under `dist/`, where the crate is not.
 */
import manifest from '../../backend/Cargo.toml?raw';

const crate = manifest.match(/^version = "([^"]+)"/m)?.[1];
if (!crate) throw new Error('backend/Cargo.toml carries no version');

export const version: string = crate;
