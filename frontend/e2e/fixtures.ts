import { readdirSync, readFileSync } from 'node:fs';
import { test as base, expect, type Page, type Request } from '@playwright/test';

const API = `${process.env.ROUTARR_E2E_URL ?? 'http://127.0.0.1:9877'}/api/v1`;

/** The stand-in Radarr the harness starts beside the server. */
const ARR = process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979';

/** The key the harness starts the server with, empty in a sign-in mode. */
const API_KEY = process.env.ROUTARR_E2E_KEY ?? '';

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
 * rules, no pinned case, no application key, no pending decision, dry-run on,
 * in English.
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
  const cases = (await api('/rule-tests?per_page=200')) as { data: { id: string }[] };
  for (const pinned of cases.data) {
    await api(`/rule-tests/${pinned.id}`, { method: 'DELETE' });
  }
  const keys = (await api('/applications')) as { id: string }[];
  for (const key of keys) {
    await api(`/applications/${key.id}`, { method: 'DELETE' });
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
  // The session a returning browser holds, opened before the first page loads,
  // through the page's own cookie jar: opened after it, the first requests
  // would land on the key gate.
  page: async ({ page }, use) => {
    if (API_KEY) {
      const opened = await page.request.post(`${API}/auth/key-session`, {
        data: { key: API_KEY },
      });
      expect(opened.ok(), `the key opened no session: ${opened.status()}`).toBe(true);
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

/**
 * Open every disclosure on the page. Folded, the body of an API operation or of
 * the facets panel is out of reach of every sweep: axe, the keyboard walk and
 * the phone width never measure what a reader sees once they open it.
 */
async function unfold(page: Page): Promise<void> {
  await page.evaluate(() => {
    for (const details of document.querySelectorAll('details')) details.open = true;
  });
}

/**
 * The writes the page sends while `act` runs and once the page has had its
 * turn. A component resumes after an answer on a later task, so a check made
 * as the dialog closes passes against one that ignores Cancel.
 */
async function writesDuring(page: Page, act: () => Promise<void>): Promise<string[]> {
  const writes: string[] = [];
  const open = new Set<Request>();
  const started = (request: Request) => {
    open.add(request);
    if (request.method() !== 'GET') {
      writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  };
  const ended = (request: Request) => open.delete(request);
  page.on('request', started);
  page.on('requestfinished', ended);
  page.on('requestfailed', ended);
  try {
    await act();
    await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 0)));
    await expect.poll(() => open.size).toBe(0);
  } finally {
    page.off('request', started);
    page.off('requestfinished', ended);
    page.off('requestfailed', ended);
  }
  return writes;
}

/**
 * Answer the proof a session gives before it makes or withdraws a key, with
 * the key the harness serves: the dialog opens above the one that asked.
 */
async function proveWithKey(page: Page): Promise<void> {
  const proof = page.getByRole('dialog', { name: 'Confirm with the API key' });
  await proof.getByLabel('Routarr API key').fill(API_KEY);
  await proof.getByRole('button', { name: 'Continue' }).click();
}

// WCAG 2.2 A and AA, the target, which `wcag22aa` completes with what 2.2 adds,
// `target-size` among it. `best-practice` on top of the standard: it is the tag
// that carries `empty-table-header`, which the WCAG tags do not, so an unnamed
// column header passes under them alone, and `landmark-one-main`.
const AXE_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice'];

export {
  expect,
  api,
  apiWhenFree,
  openScreen,
  proveWithKey,
  screenShown,
  unfold,
  writesDuring,
  API,
  ARR,
  AXE_TAGS,
};
