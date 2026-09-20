import { defineConfig, devices } from '@playwright/test';

/**
 * End-to-end tests drive the real thing: the Rust binary serving the built
 * frontend, against a fake Radarr. Nothing is mocked in the browser, so these
 * catch what the unit tests structurally cannot — a route that 404s, a button
 * wired to nothing, a table that renders empty because the payload shape moved.
 *
 * The server is started by `e2e/server.ts` rather than by `webServer` here: it
 * needs a throwaway database and a fake Arr on a known port, and both have to
 * be torn down afterwards.
 */
export default defineConfig({
  testDir: './e2e',
  // The suite drives one shared server with real state, so the tests are
  // ordered and serial on purpose: parallel runs would fight over the library.
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? 'line' : 'list',
  timeout: 30_000,
  expect: { timeout: 10_000 },
  use: {
    // The origin only. `ROUTARR_E2E_URL` may carry a mount point (`/routarr`)
    // for the API helper, but an absolute `page.goto('/rules')` would discard a
    // baseURL path anyway — so specs that need the prefix spell it out.
    baseURL: new URL(process.env.ROUTARR_E2E_URL ?? 'http://127.0.0.1:9877').origin,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
