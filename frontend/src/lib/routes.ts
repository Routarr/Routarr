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
  /** Which of the shell's counts this entry answers, if any. */
  badge?: keyof Counts;
}

/**
 * The destinations, grouped by what someone came to do.
 *
 * The order says what the application is for: what decides where a title goes
 * follows the dashboard, the screens configured once at install come after,
 * and what serves other applications comes last. A group of two to four is read
 * at a glance where a flat list is scanned every time.
 *
 * An exception sits with the rules because it outranks every one of them, and
 * the count of proposals waiting sits on the simulation, the one screen that
 * applies them.
 *
 * The group heading is a label, never a heading level: the accessibility sweep
 * checks that no screen skips one, and a navigation is not an outline.
 */
export const ROUTE_GROUPS: { key: string | null; items: Route[] }[] = [
  {
    key: null,
    items: [{ to: '/', key: 'Dashboard', hint: 'HintDashboard' }],
  },
  {
    key: 'NavGroupDaily',
    items: [
      { to: '/rules', key: 'RulesEngine', hint: 'HintRules' },
      { to: '/exceptions', key: 'Overrides', hint: 'HintOverrides' },
      { to: '/rule-tests', key: 'RuleTests', hint: 'HintRuleTests' },
      { to: '/simulation', key: 'Simulation', hint: 'HintSimulation', badge: 'decisions' },
    ],
  },
  {
    key: 'NavGroupReview',
    items: [
      { to: '/library', key: 'MediaExplorer', hint: 'HintLibrary' },
      { to: '/history', key: 'AuditHistory', hint: 'HintHistory' },
    ],
  },
  {
    key: 'NavGroupSupervision',
    items: [
      { to: '/tasks', key: 'Tasks', hint: 'HintTasks', badge: 'jobs' },
      { to: '/move-log', key: 'Logs', hint: 'HintLogs', badge: 'failed' },
      { to: '/security-log', key: 'SecurityLog', hint: 'HintSecurityLog' },
      { to: '/diagnostics', key: 'Diagnostics', hint: 'HintDiagnostics', badge: 'warnings' },
    ],
  },
  {
    key: 'NavGroupConfiguration',
    items: [
      { to: '/instances', key: 'Instances', hint: 'HintInstances' },
      { to: '/categories', key: 'RootFolders', hint: 'HintRootFolders' },
      { to: '/sources', key: 'MetadataSources', hint: 'HintSources' },
      { to: '/settings', key: 'Settings', hint: 'HintSettings' },
    ],
  },
  {
    key: 'NavGroupIntegrations',
    items: [
      { to: '/applications', key: 'Applications', hint: 'HintApplications' },
      { to: '/reference', key: 'ApiReference', hint: 'HintApiReference' },
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
