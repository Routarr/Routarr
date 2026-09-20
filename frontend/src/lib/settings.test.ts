import { describe, it, expect } from 'vitest';
import { FIELDS, SECTIONS } from './settings';

/**
 * The tab strip is the only way to reach a setting, and the payload is built
 * from *all* of `FIELDS` — so a field in no section would be unreachable and
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
   * The backend bounds every number; a field without its bounds sends `0` or
   * `-5` and gets the refusal back after Save, naming a key in a tab the
   * operator may never have opened.
   */
  it('gives every number field the bounds the backend enforces', () => {
    for (const field of FIELDS.filter((f) => f.kind === 'number')) {
      expect(field.range, field.key).toBeDefined();
      const [low, high] = field.range as [number, number | null];
      expect(low).toBeGreaterThanOrEqual(0);
      if (high !== null) expect(high).toBeGreaterThan(low);
      const fallback = Number(field.fallback);
      expect(fallback >= low && (high === null || fallback <= high), `${field.key}`).toBe(true);
    }
  });

  it('gives every field a caption and a help string to look up', () => {
    for (const field of FIELDS) {
      expect(field.labelKey, field.key).toBeTruthy();
      expect(field.helpKey, field.key).toBeTruthy();
    }
  });
});
