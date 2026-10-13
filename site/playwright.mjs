/**
 * Playwright and its axe binding, which the frontend owns and the site does not repeat.
 * A plain import resolves from `site/` and fails. `FRONTEND_DIR` overrides the path.
 */
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const FRONTEND = process.env.FRONTEND_DIR ?? fileURLToPath(new URL('../frontend', import.meta.url));

/** `require`, resolving from the frontend's `node_modules`. */
export const fromFrontend = createRequire(`${FRONTEND}/package.json`);

export const { chromium } = fromFrontend('@playwright/test');
