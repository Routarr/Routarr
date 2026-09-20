import type { Component } from 'svelte';
import type { LucideProps } from '@lucide/svelte';

import {
  Activity,
  ClipboardCheck,
  Film,
  FlaskConical,
  FolderTree,
  GitFork,
  History,
  LayoutDashboard,
  ListChecks,
  ScrollText,
  Server,
  Settings,
  ShieldAlert,
} from './icons';

import { ROUTE_GROUPS, type Route } from './routes';

export type { Counts } from './routes';

export interface NavItem extends Route {
  icon: Component<LucideProps>;
}

/**
 * The icon each destination is drawn with. Keyed by path rather than written
 * into the route table so the table stays readable without a Svelte runtime;
 * a route without an icon fails the module's test, not the first render.
 */
const ICONS: Record<string, Component<LucideProps>> = {
  '/': LayoutDashboard,
  '/rules': GitFork,
  '/rules/tests': ClipboardCheck,
  '/simulation': FlaskConical,
  '/media': Film,
  '/history': History,
  '/overrides': ShieldAlert,
  '/jobs': ListChecks,
  '/logs': ScrollText,
  '/health': Activity,
  '/instances': Server,
  '/root-folders': FolderTree,
  '/settings': Settings,
};

function iconFor(route: Route): Component<LucideProps> {
  const icon = ICONS[route.to];
  if (!icon) throw new Error(`navigation: no icon for ${route.to}`);
  return icon;
}

/**
 * The route table, dressed for the sidebar and the palette. Here rather than
 * inside the sidebar because two things read it: the navigation draws it and
 * the command palette searches it, and two copies of a route table is how a
 * destination ends up reachable from one and not the other.
 */
export const GROUPS: { key: string | null; items: NavItem[] }[] = ROUTE_GROUPS.map((group) => ({
  key: group.key,
  items: group.items.map((route) => ({ ...route, icon: iconFor(route) })),
}));

/** Every destination, flat — what a search over the navigation reads. */
export const DESTINATIONS: NavItem[] = GROUPS.flatMap((group) => group.items);
