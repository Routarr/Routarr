import type { Component } from 'svelte';
import type { LucideProps } from '@lucide/svelte';

import {
  Activity,
  BookOpen,
  ClipboardCheck,
  Database,
  Film,
  FlaskConical,
  FolderTree,
  GitFork,
  History,
  LayoutDashboard,
  ListChecks,
  Plug,
  ScrollText,
  Server,
  Settings,
  ShieldAlert,
  ShieldCheck,
} from './icons';

import { ROUTE_GROUPS, type Route } from './routes';

export type { Counts } from './routes';

interface NavItem extends Route {
  icon: Component<LucideProps>;
}

interface NavGroup {
  key: string | null;
  items: NavItem[];
}

/**
 * The icon each destination is drawn with. Keyed by path rather than written
 * into the route table so the table stays readable without a Svelte runtime.
 * A route without an icon fails the module's test, not the first render.
 */
const ICONS: Record<string, Component<LucideProps>> = {
  '/': LayoutDashboard,
  '/rules': GitFork,
  '/exceptions': ShieldAlert,
  '/rule-tests': ClipboardCheck,
  '/simulation': FlaskConical,
  '/library': Film,
  '/history': History,
  '/tasks': ListChecks,
  '/move-log': ScrollText,
  '/security-log': ShieldCheck,
  '/diagnostics': Activity,
  '/instances': Server,
  '/categories': FolderTree,
  '/sources': Database,
  '/settings': Settings,
  '/applications': Plug,
  '/reference': BookOpen,
};

/** A screen's icon. Exported for the tests, which hold every route to one. */
export function iconFor(route: Route): Component<LucideProps> {
  const icon = ICONS[route.to];
  if (!icon) throw new Error(`navigation: no icon for ${route.to}`);
  return icon;
}

/** The route table with its icons, which the sidebar draws and the palette searches. */
const GROUPS: NavGroup[] = ROUTE_GROUPS.map((group) => ({
  key: group.key,
  items: group.items.map((route) => ({ ...route, icon: iconFor(route) })),
}));

/** The groups offered to the owner, or to a reader, who is offered no screen of the owner's. */
export function groupsFor(readOnly: boolean): NavGroup[] {
  if (!readOnly) return GROUPS;
  return GROUPS.map((group) => ({
    key: group.key,
    items: group.items.filter((item) => !item.owner),
  })).filter((group) => group.items.length > 0);
}

/** Every destination offered, flat: what a search over the navigation reads. */
export const destinationsFor = (readOnly: boolean): NavItem[] =>
  groupsFor(readOnly).flatMap((group) => group.items);
