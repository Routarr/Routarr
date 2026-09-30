import { readdirSync, readFileSync } from 'node:fs';
import { test as base, expect, type Page } from '@playwright/test';

const API = `${process.env.ROUTARR_E2E_URL ?? 'http://127.0.0.1:9877'}/api/v1`;

/** The stand-in Radarr the harness starts beside the server. */
const ARR = process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979';

/** The key the harness starts the server with, empty in a sign-in mode. */
const API_KEY = process.env.ROUTARR_E2E_KEY ?? '';

/** Where the frontend keeps the key it sends with every request. */
const API_KEY_STORAGE = 'routarr.apiKey';

function send(path: string, init?: RequestInit): Promise<Response> {
  return fetch(`${API}${path}`, {
    ...init,
    headers: {
      'content-type': 'application/json',
      ...(API_KEY ? { 'x-api-key': API_KEY } : {}),
      ...(init?.headers ?? {}),
    },
  });
}

async function read(response: Response, path: string, init?: RequestInit): Promise<unknown> {
  if (!response.ok) {
    throw new Error(
      `${init?.method ?? 'GET'} ${path} → ${response.status} ${await response.text()}`,
    );
  }
  return response.status === 204 ? null : response.json();
}

async function api(path: string, init?: RequestInit): Promise<unknown> {
  return read(await send(path, init), path, init);
}

/**
 * `api`, for a pass the scheduler may be running already: sent again once that
 * one has ended. The scheduler syncs a new instance at its next pass and
 * simulates after it, and a second sync of one instance, or a second
 * simulation, answers 409 until the first one ends. Waiting is a readiness
 * step here, never a claim about the refusal.
 */
async function apiWhenFree(path: string, init: RequestInit): Promise<unknown> {
  const deadline = Date.now() + 20_000;
  let response = await send(path, init);
  while (response.status === 409 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    response = await send(path, init);
  }
  return read(response, path, init);
}

/**
 * Put the instance back the way every test expects to find it: one Radarr, its
 * root folders mapped to `standard` and `anime` and no other category, no
 * rules, no pinned case, no pending decision, dry-run on, in English.
 *
 * Done through the API rather than against the database, so a reset that the
 * API cannot express is a reset the product cannot express either.
 */
async function resetLibrary(): Promise<string> {
  // The fake Arr keeps its library in memory and the tests mutate it, so a run
  // that moved a film would otherwise leave the next one with nothing to move.
  await fetch(`${ARR}/__reset`);

  const instances = (await api('/instances')) as { id: string }[];
  for (const instance of instances) {
    await api(`/instances/${instance.id}`, { method: 'DELETE' });
  }
  const rules = (await api('/rules')) as { id: string }[];
  for (const rule of rules) {
    await api(`/rules/${rule.id}`, { method: 'DELETE' });
  }
  // A case keeps its own copy of the film, so it outlives the instance.
  const cases = (await api('/rule-tests')) as { id: string }[];
  for (const pinned of cases) {
    await api(`/rule-tests/${pinned.id}`, { method: 'DELETE' });
  }
  // Nothing refers to a category once the rules and the folders are gone, so
  // one a test created or renamed can go.
  const categories = (await api('/categories')) as {
    id: string;
    name: string;
    is_default: boolean;
  }[];
  for (const category of categories) {
    if (!category.is_default && category.name !== 'anime') {
      await api(`/categories/${category.id}`, { method: 'DELETE' });
    }
  }
  if (!categories.some((c) => c.name === 'anime')) {
    await api('/categories', { method: 'POST', body: JSON.stringify({ name: 'anime' }) });
  }

  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({
      settings: {
        global_dry_run: 'true',
        auto_apply_enabled: 'false',
        ui_language: 'en',
        batch_limit: '50',
        confirmation_threshold: '10',
        // Every journey but the guide's tests an installation already set up.
        onboarding: 'done',
      },
    }),
  });

  const created = (await api('/instances', {
    method: 'POST',
    body: JSON.stringify({
      name: 'Radarr',
      instance_type: 'radarr',
      base_url: ARR,
      api_key: 'e2e-key',
      enabled: true,
      sync_interval_minutes: 60,
    }),
  })) as { id: string };

  await apiWhenFree(`/instances/${created.id}/sync`, { method: 'POST' });

  for (const [arrId, category] of [
    [1, 'standard'],
    [2, 'anime'],
  ] as const) {
    await api(`/root-folders/rf-${created.id}-${arrId}/category`, {
      method: 'PUT',
      body: JSON.stringify({ category }),
    });
  }

  return created.id;
}

/** The not-found page's heading, in every language the backend ships. */
const NOT_FOUND = (() => {
  const locales = new URL('../../backend/locales/', import.meta.url);
  const titles = readdirSync(locales)
    .filter((file) => file.endsWith('.json'))
    .map((file) => {
      const strings = JSON.parse(readFileSync(new URL(file, locales), 'utf-8')) as {
        NotFoundTitle: string;
      };
      return strings.NotFoundTitle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    });
  return new RegExp(`^(${titles.join('|')})$`);
})();

/**
 * Wait for the screen itself, not merely for the URL, and fail on the
 * not-found page.
 *
 * Each route is its own chunk, so a navigation ends with the fallback on
 * screen and the page's own markup still in flight. A path the shell does not
 * route draws a heading too, the not-found page's, and a sweep that took it
 * for the screen it asked for would measure the wrong page and pass.
 */
async function screenShown(page: Page, where: string): Promise<void> {
  const heading = page.locator('h1').first();
  await expect(heading, `${where} draws no heading`).toBeVisible();
  await expect(heading, `${where} is the not-found page`).not.toHaveText(NOT_FOUND);
}

/** Open a screen and wait for it, as `screenShown` does. */
async function openScreen(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await screenShown(page, path);
}

/**
 * Every test starts from the known library, with the browser authenticated.
 *
 * The reset runs whether a test asks for the instance or not: the suite shares
 * one server, and a test that leaves a setting behind would otherwise decide
 * the language or the mode of the next one. In a sign-in mode the harness
 * holds no key to reset with, and `instanceId` is empty.
 */
export const test = base.extend<{ instanceId: string }>({
  // Seeded before any script runs, exactly as a returning user's browser would
  // have it: the frontend reads the key from storage on its first request, so
  // setting it afterwards would leave the initial page load unauthenticated.
  page: async ({ page }, use) => {
    if (API_KEY) {
      await page.addInitScript(
        ([storageKey, value]) => window.localStorage.setItem(storageKey, value),
        [API_KEY_STORAGE, API_KEY] as const,
      );
    }
    await use(page);
  },
  instanceId: [
    async ({}, use) => {
      await use(API_KEY ? await resetLibrary() : '');
    },
    { auto: true },
  ],
});

export { expect, api, apiWhenFree, openScreen, screenShown, ARR };
