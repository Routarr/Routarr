/**
 * Playwright and its axe binding, loaded from the frontend, which owns them:
 * the site does not repeat the dependency. ESM resolves an import from the
 * importing file's directory, so a plain import would look under `site/` and
 * fail however the script is launched. `FRONTEND_DIR` points elsewhere.
 */
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const FRONTEND = process.env.FRONTEND_DIR ?? fileURLToPath(new URL('../frontend', import.meta.url));

/** `require`, resolving from the frontend's `node_modules`. */
export const fromFrontend = createRequire(`${FRONTEND}/package.json`);

export const { chromium } = fromFrontend('@playwright/test');
