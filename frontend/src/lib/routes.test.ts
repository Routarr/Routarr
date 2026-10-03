import { describe, it, expect } from 'vitest';
import { SCREENS, screenKey } from './routes';
import { iconFor } from './navigation';

/**
 * The route table is data so the e2e sweeps can read it, and the icons are
 * looked up beside it. The two have to agree, or a destination renders without
 * a glyph. `navigation.ts` dresses the table as it loads, so a route with no
 * icon fails the import of this file before any test runs.
 */
describe('the route table', () => {
  it('names distinct screens, the dashboard first', () => {
    expect(SCREENS.length).toBeGreaterThan(5);
    expect(new Set(SCREENS).size).toBe(SCREENS.length);
    expect(SCREENS[0]).toBe('/');
    for (const screen of SCREENS) expect(screen).toMatch(/^\//);
  });

  /** The router leaves `/api` to the server, so a screen there is unreachable. */
  it('puts no screen under /api', () => {
    for (const screen of SCREENS) expect(screen).not.toMatch(/^\/api(\/|$)/);
  });

  it('refuses to dress a destination it has no icon for, naming it', () => {
    expect(() => iconFor({ to: '/nowhere', key: 'Nowhere', hint: 'HintNowhere' })).toThrow(
      '/nowhere',
    );
  });
});

describe('screenKey', () => {
  it('names a screen by its menu entry, and any other path as not found', () => {
    expect(screenKey('/rules/tests')).toBe('RuleTests');
    expect(screenKey('/')).toBe('Dashboard');
    expect(screenKey('/rules/nope')).toBe('NotFoundTitle');
  });
});
