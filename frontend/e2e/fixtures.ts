import { test as base, expect } from '@playwright/test';

const API = `${process.env.ROUTARR_E2E_URL ?? 'http://127.0.0.1:9877'}/api/v1`;

/** The key the harness starts the server with; empty when it runs open. */
const API_KEY = process.env.ROUTARR_E2E_KEY ?? '';

/** Where the frontend keeps the key it sends with every request. */
const API_KEY_STORAGE = 'routarr.apiKey';

async function api(path: string, init?: RequestInit): Promise<unknown> {
  const response = await fetch(`${API}${path}`, {
    ...init,
    headers: {
      'content-type': 'application/json',
      ...(API_KEY ? { 'x-api-key': API_KEY } : {}),
      ...(init?.headers ?? {}),
    },
  });
  if (!response.ok) {
    throw new Error(
      `${init?.method ?? 'GET'} ${path} → ${response.status} ${await response.text()}`,
    );
  }
  return response.status === 204 ? null : response.json();
}

/**
 * Put the instance back the way every test expects to find it: one Radarr, its
 * root folders mapped, no rules, no decisions, dry-run on.
 *
 * Done through the API rather than against the database, so a reset that the
 * API cannot express is a reset the product cannot express either.
 */
async function resetLibrary(): Promise<string> {
  // The fake Arr keeps its library in memory and the tests mutate it, so a run
  // that moved a film would otherwise leave the next one with nothing to move.
  await fetch(`${process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979'}/__reset`);

  const instances = (await api('/instances')) as { id: string }[];
  for (const instance of instances) {
    await api(`/instances/${instance.id}`, { method: 'DELETE' });
  }
  const rules = (await api('/rules')) as { id: string }[];
  for (const rule of rules) {
    await api(`/rules/${rule.id}`, { method: 'DELETE' });
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
      },
    }),
  });

  const created = (await api('/instances', {
    method: 'POST',
    body: JSON.stringify({
      name: 'Radarr',
      instance_type: 'radarr',
      base_url: process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979',
      api_key: 'e2e-key',
      enabled: true,
      sync_interval_minutes: 60,
    }),
  })) as { id: string };

  await api(`/instances/${created.id}/sync`, { method: 'POST' });

  const categories = (await api('/categories')) as { name: string }[];
  if (!categories.some((c) => c.name === 'anime')) {
    await api('/categories', { method: 'POST', body: JSON.stringify({ name: 'anime' }) });
  }
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

/** A test that starts from a known library, with the browser authenticated. */
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
  instanceId: async ({}, use) => {
    await use(await resetLibrary());
  },
});

export { expect, api };
