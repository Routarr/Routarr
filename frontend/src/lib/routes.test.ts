import { describe, it, expect } from 'vitest';
import { ROUTE_GROUPS, SCREENS } from './routes';
import { DESTINATIONS, GROUPS } from './navigation';

/**
 * The route table is data so the e2e sweeps can read it; the icons are looked
 * up beside it. The two have to agree, or a destination renders without a
 * glyph — or throws on the first render, which this catches first.
 */
describe('the route table', () => {
  it('names thirteen distinct screens, the dashboard first', () => {
    expect(SCREENS).toHaveLength(13);
    expect(new Set(SCREENS).size).toBe(13);
    expect(SCREENS[0]).toBe('/');
    for (const screen of SCREENS) expect(screen).toMatch(/^\//);
  });

  it('is dressed with an icon for every destination and nothing lost', () => {
    expect(DESTINATIONS).toHaveLength(SCREENS.length);
    expect(GROUPS.map((group) => group.key)).toEqual(ROUTE_GROUPS.map((group) => group.key));
    for (const item of DESTINATIONS) expect(item.icon, item.to).toBeDefined();
  });
});
