import { describe, it, expect } from 'vitest';
import { FIELDS, SECTIONS } from './settings';
import { readBackend } from '../test/backend';

/**
 * The tab strip is the only way to reach a setting, and the payload is built
 * from *all* of `FIELDS`, so a field in no section would be unreachable and
 * saved with its fallback, silently, on every save anyone ever made.
 */
describe('the settings grouping', () => {
  it('covers every field exactly once', () => {
    const grouped = SECTIONS.flatMap((section) => section.keys as readonly string[]);
    const declared = FIELDS.map((field) => field.key);

    expect([...grouped].sort()).toEqual([...declared].sort());
  });

  it('puts no field in two sections', () => {
    const grouped = SECTIONS.flatMap((section) => section.keys as readonly string[]);
    expect(new Set(grouped).size).toBe(grouped.length);
  });

  /**
   * `global_dry_run` and `auto_apply_enabled` are the pair that arms unattended
   * writing, and they live in different sections. That is what makes the save
   * bar's "every section is saved together" load-bearing rather than decorative.
   */
  it('keeps the two switches that arm unattended writing in different sections', () => {
    const sectionOf = (key: string) =>
      SECTIONS.find((section) => (section.keys as readonly string[]).includes(key))?.id;

    expect(sectionOf('global_dry_run')).toBeDefined();
    expect(sectionOf('auto_apply_enabled')).toBeDefined();
    expect(sectionOf('global_dry_run')).not.toBe(sectionOf('auto_apply_enabled'));
  });

  /**
   * The backend bounds every number. A field whose bounds part from the
   * server's sends a value it refuses, and the refusal comes back after Save,
   * naming a key in a tab the operator may never have opened.
   */
  it('gives every number field the bounds the backend enforces', () => {
    const bounds = backendBounds();
    expect(bounds.size).toBeGreaterThan(8);
    const numbers = FIELDS.filter((f) => f.kind === 'number');
    expect(numbers.map((field) => field.key).sort()).toEqual([...bounds.keys()].sort());
    for (const field of numbers) {
      expect(field.range, field.key).toEqual(bounds.get(field.key));
      const [low, high] = field.range as [number, number];
      const fallback = Number(field.fallback);
      expect(fallback >= low && fallback <= high, field.key).toBe(true);
    }
  });

  it('gives every field a caption and a help string to look up', () => {
    for (const field of FIELDS) {
      expect(field.labelKey, field.key).toBeTruthy();
      expect(field.helpKey, field.key).toBeTruthy();
    }
  });
});

/** A file of the backend's sources, read as text. */
function backend(...parts: string[]): string {
  return readBackend(`backend/src/${parts.join('/')}`);
}

/**
 * Each ranged setting's bounds as `KNOWN` in `services/settings.rs` declares
 * them, `Kind::Bounded` and `Kind::Retention` alike, a named ceiling read
 * where the file writes it.
 */
function backendBounds(): Map<string, [number, number]> {
  const table = backend('services', 'settings.rs');
  const number = (written: string): number => {
    const named = new RegExp(`const ${written.trim()}: i64 = ([\\d_ *]+);`).exec(table);
    return (named?.[1] ?? written)
      .split('*')
      .reduce((product, factor) => product * Number(factor.trim().replaceAll('_', '')), 1);
  };
  const bounds = new Map<string, [number, number]>();
  const ranged = /^\s+\("([a-z_]+)", Kind::(?:Bounded|Retention)\(([^,]+), ([^)]+)\)/gm;
  for (const [, key, low, high] of table.matchAll(ranged)) {
    if (key !== undefined && low !== undefined && high !== undefined) {
      bounds.set(key, [number(low), number(high)]);
    }
  }
  return bounds;
}

/**
 * Each setting's default as `KNOWN` in `services/settings.rs` declares it,
 * the constants it names read where they are written.
 */
function backendDefaults(): Map<string, string> {
  const constants = new Map<string, string>();
  for (const [file, name] of [
    [['state.rs'], 'DEFAULT_CATEGORY'],
    [['services', 'metadata.rs'], 'ARR'],
    [['localization.rs'], 'DEFAULT_LANGUAGE'],
  ] as const) {
    const found = new RegExp(`pub const ${name}: &str = "([^"]*)";`).exec(backend(...file));
    if (found?.[1] !== undefined) constants.set(name, found[1]);
  }
  const search = /pub const ANIME_SEARCH: \[&str; \d+\] = \["([^"]*)"/.exec(
    backend('services', 'metadata.rs'),
  );
  if (search?.[1] !== undefined) constants.set('ANIME_SEARCH[0]', search[1]);

  const table = backend('services', 'settings.rs');
  const defaults = new Map<string, string>();
  for (const entry of table.matchAll(/^\s+\("([a-z_]+)", Kind::[^\n]*, ([^\n]+)\),$/gm)) {
    const [, key, written] = entry;
    if (key === undefined || written === undefined) continue;
    const literal = /^"([^"]*)"$/.exec(written);
    const named = written.split('::').pop() ?? '';
    defaults.set(key, literal?.[1] ?? constants.get(named) ?? `unresolved ${written}`);
  }
  return defaults;
}

/**
 * A field shows the backend's own default while nothing is stored, since a
 * setting never saved is left out of `GET /settings`. Written twice, the two
 * copies would part the day one of them changed, and the screen would show a
 * value the server does not apply.
 */
describe('each fallback', () => {
  it("is the backend's default", () => {
    const defaults = backendDefaults();
    expect(defaults.size).toBeGreaterThan(25);
    for (const field of FIELDS) {
      expect(defaults.has(field.key), `${field.key} is not a setting the backend knows`).toBe(true);
      expect(field.fallback, field.key).toBe(defaults.get(field.key));
    }
  });
});
