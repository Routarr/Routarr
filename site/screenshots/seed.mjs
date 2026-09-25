/**
 * Seed a throwaway Routarr with a library that looks like somebody's actual
 * collection, so the showcase screenshots show the engine doing real work.
 *
 * Never points at a development database: `run.sh` gives it its own.
 */
const BASE = process.env.ROUTARR_URL ?? 'http://127.0.0.1:9899';
const API = `${BASE}/api/v1`;

const API_KEY = process.env.ROUTARR_API_KEY ?? '';

async function api(path, init) {
  const response = await fetch(`${API}${path}`, {
    ...init,
    headers: {
      'content-type': 'application/json',
      ...(API_KEY ? { 'x-api-key': API_KEY } : {}),
      ...(init?.headers ?? {}),
    },
  });
  if (!response.ok) {
    throw new Error(`${init?.method ?? 'GET'} ${path} → ${response.status} ${await response.text()}`);
  }
  return response.status === 204 ? null : response.json();
}

// The ports run.sh started the fakes on, which stay clear of the e2e harness's.
const INSTANCES = [
  { name: 'Radarr', instance_type: 'radarr', port: Number(process.env.RADARR_PORT), folders: { 1: 'standard', 2: 'anime', 3: 'kids', 4: 'concerts' } },
  { name: 'Sonarr', instance_type: 'sonarr', port: Number(process.env.SONARR_PORT), folders: { 1: 'standard', 2: 'anime', 3: 'documentaries' } },
];
if (!INSTANCES.every((instance) => instance.port > 0)) {
  throw new Error('RADARR_PORT and SONARR_PORT name the fakes run.sh started');
}

const CATEGORIES = ['standard', 'anime', 'kids', 'concerts', 'documentaries'];

const RULES = [
  {
    name: 'Japanese animation',
    description: 'Anime, unless it is clearly aimed at small children.',
    target_category: 'anime',
    media_type: 'both',
    match_mode: 'all',
    priority: 10,
    enabled: true,
    conditions: [
      { type: 'original_language', value: ['ja'] },
      { type: 'genre_contains', value: ['Animation'] },
    ],
    exclusions: [{ type: 'certification_in', value: ['G', 'U', 'TV-Y'] }],
  },
  {
    name: 'Concert films',
    description: 'A music genre alone is not enough — Whiplash is not a concert.',
    target_category: 'concerts',
    media_type: 'movie',
    match_mode: 'all',
    priority: 20,
    enabled: true,
    conditions: [
      { type: 'genre_contains', value: ['Music'] },
      { type: 'keyword_contains', value: ['concert', 'live performance', 'tour'] },
    ],
    exclusions: [],
  },
  {
    name: 'Family and kids',
    description: 'Only movies: this library keeps no kids folder on Sonarr.',
    target_category: 'kids',
    media_type: 'movie',
    match_mode: 'any',
    priority: 30,
    enabled: true,
    conditions: [
      { type: 'genre_contains', value: ['Family'] },
      { type: 'certification_in', value: ['G', 'U', 'TV-Y', 'TV-G'] },
    ],
    exclusions: [],
  },
  {
    name: 'Documentaries',
    target_category: 'documentaries',
    media_type: 'series',
    match_mode: 'all',
    priority: 40,
    enabled: true,
    conditions: [{ type: 'genre_contains', value: ['Documentary'] }],
    exclusions: [],
  },
];

const existing = await api('/categories');
for (const name of CATEGORIES) {
  if (!existing.some((c) => c.name === name)) {
    await api('/categories', { method: 'POST', body: JSON.stringify({ name }) });
  }
}

for (const instance of INSTANCES) {
  const created = await api('/instances', {
    method: 'POST',
    body: JSON.stringify({
      name: instance.name,
      instance_type: instance.instance_type,
      base_url: `http://127.0.0.1:${instance.port}`,
      api_key: 'demo-key-not-a-real-one',
      enabled: true,
      sync_interval_minutes: 60,
    }),
  });

  await api(`/instances/${created.id}/sync`, { method: 'POST' });

  for (const [arrId, category] of Object.entries(instance.folders)) {
    await api(`/root-folders/rf-${created.id}-${arrId}/category`, {
      method: 'PUT',
      body: JSON.stringify({ category }),
    });
  }
}

for (const rule of RULES) {
  await api('/rules', { method: 'POST', body: JSON.stringify(rule) });
}

await api('/settings', {
  method: 'PUT',
  body: JSON.stringify({
    settings: {
      global_dry_run: 'true',
      ui_language: 'en',
      default_category: 'standard',
      scheduler_interval_minutes: '15',
    },
  }),
});

console.log('seeded');
