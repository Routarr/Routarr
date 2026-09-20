/**
 * Where the shell can go, without the icons.
 *
 * The icons are Svelte components, and a module that imports them cannot be
 * read by anything without a Svelte runtime — which is what kept the e2e
 * sweeps on lists of their own, kept by hand, one of them missing a screen.
 * This file is plain data: `navigation.ts` dresses it in icons for the
 * sidebar and the palette, and `e2e/screens.ts` reads it as it is.
 */

/**
 * What the shell counts, and therefore what an entry may carry.
 *
 * The top bar stated these as four translated sentences; each now sits on the
 * destination that answers it, where it can be acted on.
 */
export interface Counts {
  jobs: number;
  decisions: number;
  failed: number;
  warnings: number;
}

export interface Route {
  to: string;
  /** Dictionary key for the label, the title and the accessible name. */
  key: string;
  /** Set where a nested route would otherwise light its parent too. */
  exact?: boolean;
  /** Which of the shell's counts this entry answers, if any. */
  badge?: keyof Counts;
}

/**
 * Thirteen destinations, grouped by what someone came to do.
 *
 * Flat, they sat in the order they were built: the two screens configured once
 * at install held the best positions and the rules — the screen the product
 * exists for — came fourth. Grouped, the order says what the application is
 * for, and a group of two or three is read at a glance where a list of
 * thirteen is scanned every time.
 *
 * The group heading is a label, never a heading level: the accessibility sweep
 * checks that no screen skips one, and a navigation is not an outline.
 */
export const ROUTE_GROUPS: { key: string | null; items: Route[] }[] = [
  {
    key: null,
    items: [{ to: '/', key: 'Dashboard', exact: true }],
  },
  {
    key: 'NavGroupDaily',
    items: [
      // Exact: `/rules/tests` is its own destination, and without this both
      // would light up at once.
      { to: '/rules', key: 'RulesEngine', exact: true },
      { to: '/rules/tests', key: 'RuleTests' },
      { to: '/simulation', key: 'Simulation' },
    ],
  },
  {
    key: 'NavGroupReview',
    items: [
      { to: '/media', key: 'MediaExplorer' },
      { to: '/history', key: 'AuditHistory', badge: 'decisions' },
      { to: '/overrides', key: 'Overrides' },
    ],
  },
  {
    key: 'NavGroupSupervision',
    items: [
      { to: '/jobs', key: 'Tasks', badge: 'jobs' },
      { to: '/logs', key: 'Logs', badge: 'failed' },
      { to: '/health', key: 'Diagnostics', badge: 'warnings' },
    ],
  },
  {
    key: 'NavGroupConfiguration',
    items: [
      { to: '/instances', key: 'Instances' },
      { to: '/root-folders', key: 'RootFolders' },
      { to: '/settings', key: 'Settings' },
    ],
  },
];

/** Every screen the shell reaches, in the order the navigation shows them. */
export const SCREENS: string[] = ROUTE_GROUPS.flatMap((group) =>
  group.items.map((item) => item.to),
);
