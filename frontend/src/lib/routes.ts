/**
 * Where the shell can go, without the icons.
 *
 * The icons are Svelte components, and a module that imports them cannot be
 * read by anything without a Svelte runtime, such as the e2e sweeps. This file
 * is plain data: `navigation.ts` dresses it in icons for the sidebar and the
 * palette, and `e2e/screens.ts` reads it as it is, so no sweep keeps a list of
 * screens by hand.
 *
 * A new screen also needs its lazy import in `ROUTES` (`App.svelte`) and its
 * icon in `ICONS` (`lib/navigation.ts`), which `test/layout.test.ts` and
 * `lib/routes.test.ts` hold together.
 */

/**
 * What the shell counts, and therefore what an entry may carry. Each count sits
 * on the destination that answers it, where it can be acted on.
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
  /** Dictionary key for the few words the quick search shows beside the label. */
  hint: string;
  /** Set where a nested route would otherwise light its parent too. */
  exact?: boolean;
  /** Which of the shell's counts this entry answers, if any. */
  badge?: keyof Counts;
}

/**
 * The destinations, grouped by what someone came to do.
 *
 * The order says what the application is for: the rules, the screen the
 * product exists for, follow the dashboard, and the screens configured once at
 * install come last. A group of two or three is read at a glance where a flat
 * list is scanned every time.
 *
 * The group heading is a label, never a heading level: the accessibility sweep
 * checks that no screen skips one, and a navigation is not an outline.
 */
export const ROUTE_GROUPS: { key: string | null; items: Route[] }[] = [
  {
    key: null,
    items: [{ to: '/', key: 'Dashboard', hint: 'HintDashboard', exact: true }],
  },
  {
    key: 'NavGroupDaily',
    items: [
      // Exact: `/rules/tests` is its own destination, and without this both
      // would light up at once.
      { to: '/rules', key: 'RulesEngine', hint: 'HintRules', exact: true },
      { to: '/rules/tests', key: 'RuleTests', hint: 'HintRuleTests' },
      { to: '/simulation', key: 'Simulation', hint: 'HintSimulation' },
    ],
  },
  {
    key: 'NavGroupReview',
    items: [
      { to: '/media', key: 'MediaExplorer', hint: 'HintLibrary' },
      { to: '/history', key: 'AuditHistory', hint: 'HintHistory', badge: 'decisions' },
      { to: '/overrides', key: 'Overrides', hint: 'HintOverrides' },
    ],
  },
  {
    key: 'NavGroupSupervision',
    items: [
      { to: '/jobs', key: 'Tasks', hint: 'HintTasks', badge: 'jobs' },
      { to: '/logs', key: 'Logs', hint: 'HintLogs', badge: 'failed' },
      { to: '/health', key: 'Diagnostics', hint: 'HintDiagnostics', badge: 'warnings' },
    ],
  },
  {
    key: 'NavGroupConfiguration',
    items: [
      { to: '/instances', key: 'Instances', hint: 'HintInstances' },
      { to: '/root-folders', key: 'RootFolders', hint: 'HintRootFolders' },
      { to: '/applications', key: 'Applications', hint: 'HintApplications' },
      { to: '/reference', key: 'ApiReference', hint: 'HintApiReference' },
      { to: '/settings', key: 'Settings', hint: 'HintSettings' },
    ],
  },
];

/** Every screen the shell reaches, in the order the navigation shows them. */
export const SCREENS: string[] = ROUTE_GROUPS.flatMap((group) =>
  group.items.map((item) => item.to),
);

/** The dictionary key naming the screen at `path`: its menu entry's, or the not-found page's. */
export const screenKey = (path: string): string =>
  ROUTE_GROUPS.flatMap((group) => group.items).find((item) => item.to === path)?.key ??
  'NotFoundTitle';
