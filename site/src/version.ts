/**
 * Routarr's version, as the page states it: the latest published release, or
 * the crate's when GitHub cannot say (`release.mjs`).
 *
 * The crate is imported as text so it is inlined at bundle time: a path read
 * at prerender resolves under `dist/`, where the crate is not.
 */
import manifest from '../../backend/Cargo.toml?raw';
import { releasedVersion } from '../release.mjs';

const crate = manifest.match(/^version = "([^"]+)"/m)?.[1];
if (!crate) throw new Error('backend/Cargo.toml carries no version');

export const version: string = await releasedVersion(crate);
