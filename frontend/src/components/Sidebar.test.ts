import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';

import { SCREENS } from '../lib/routes';
import { renderWithI18n } from '../test/render';
import { href, navigate } from '../lib/router.svelte';
import { withBase } from '../test/base';
import { click } from '../test/links';
import Sidebar from './Sidebar.svelte';

/**
 * Below the rail breakpoint the labels are hidden. The name has to reach a
 * screen reader, and a pointer, some other way, or the navigation becomes a
 * column of unnamed icons.
 */

const STRINGS = {
  Dashboard: 'Dashboard',
  Instances: 'Instances',
  RootFolders: 'Categories and folders',
  MetadataSources: 'Metadata sources',
  Applications: 'Applications',
  ApiReference: 'API reference',
  RulesEngine: 'Rules',
  RuleTests: 'Rule tests',
  MainNavigation: 'Main navigation',
  NavGroupDaily: 'Routing',
  NavGroupReview: 'Review',
  NavGroupSupervision: 'Monitoring',
  NavGroupConfiguration: 'Configuration',
  NavGroupIntegrations: 'Integrations',
  DiagnosticWarnings: '{count} warnings',
  TasksRunning: '{count} running',
  DecisionsAwaitingReview: '{count} awaiting review',
  FailedMoves: '{count} failed',
  MediaExplorer: 'Media',
  Simulation: 'Simulation',
  AuditHistory: 'History',
  Overrides: 'Exceptions',
  Tasks: 'Tasks',
  Logs: 'Move log',
  Diagnostics: 'Diagnostics',
  Settings: 'Settings',
};

const show = (props: Record<string, unknown> = {}) =>
  renderWithI18n(Sidebar, { props, strings: STRINGS });

afterEach(() => {
  withBase(null);
  vi.restoreAllMocks();
});

describe('Sidebar', () => {
  it('names every destination, even when the rail hides the label', async () => {
    show();

    const links = await screen.findAllByRole('link');
    // Not the claim (the loop below is), but without it the loop passes over
    // an empty list and asserts nothing at all.
    expect(links).toHaveLength(SCREENS.length);
    for (const link of links) {
      expect(link.getAttribute('title')).toBeTruthy();
    }
  });

  /**
   * Marking the active entry by colour alone says nothing to a screen reader
   * and disappears for anyone who cannot separate the two greys.
   */
  it('announces which entry is current, rather than only colouring it', async () => {
    navigate('/rules');
    show();

    const current = await screen.findByRole('link', { current: 'page' });
    expect(current.textContent).toContain('Rules');
  });

  /**
   * The path of Rules starts the path of Rule tests. `findByRole` throws on two
   * current links.
   */
  it('marks only Rule tests current on its own screen', async () => {
    navigate('/rule-tests');
    show();

    const current = await screen.findByRole('link', { current: 'page' });
    expect(current.textContent).toContain('Rule tests');
  });

  it('closes the drawer when a destination is chosen', async () => {
    const onNavigate = vi.fn();
    show({ open: true, onNavigate });

    click(await screen.findByRole('link', { name: /Move log/ }));

    expect(onNavigate).toHaveBeenCalledTimes(1);
  });

  /**
   * The order is the argument: what decides where a title goes comes first,
   * the exceptions beside the rules they outrank, then what is configured once
   * at install, and what serves other applications last.
   */
  it('opens on routing and ends on the integrations', () => {
    show();

    const links = screen.getAllByRole('link').map((link) => link.getAttribute('href'));
    expect(links.slice(0, 5)).toEqual(['/', '/rules', '/exceptions', '/rule-tests', '/simulation']);
    expect(links.slice(-6)).toEqual([
      '/instances',
      '/categories',
      '/sources',
      '/settings',
      '/applications',
      '/reference',
    ]);
  });

  /**
   * A group of two or three is read at a glance where a long list is scanned
   * every time, but only if a reader is told the groups exist.
   */
  it('names every group, and the navigation itself', () => {
    show();

    expect(screen.getByRole('navigation', { name: 'Main navigation' })).toBeInTheDocument();
    for (const group of ['Routing', 'Review', 'Monitoring', 'Configuration', 'Integrations']) {
      expect(screen.getByRole('list', { name: group })).toBeInTheDocument();
    }
  });

  /**
   * A `<base href>` applies to relative URLs only. Left root-absolute without
   * the mount point, a link sends a click, a middle-click, a ctrl-click or a
   * copied link to the proxy's root.
   */
  it('prefixes every destination with the mount point a reverse proxy adds', () => {
    withBase('/routarr/');
    show({ counts: { jobs: 1, decisions: 0, failed: 0, warnings: 0 } });

    const links = screen.getAllByRole('link').map((a) => a.getAttribute('href'));
    expect(links.length).toBeGreaterThanOrEqual(SCREENS.length);
    for (const link of links) expect(link).toMatch(/^\/routarr\//);
    expect(links).toContain('/routarr/rules');
    expect(links).toContain('/routarr/');
  });

  /**
   * In the rail the badge is the only thing that says something is waiting,
   * and a zero must not draw the eye to nothing.
   */
  it('carries each count onto the entry that answers it, and nothing when there is none', () => {
    const { unmount } = show({
      counts: { jobs: 2, decisions: 5, failed: 1, warnings: 3 },
    });

    // The sentence is part of the link's name: an `aria-label` on a generic
    // span reaches nobody reliably.
    const entry = (sentence: string) => screen.getByText(sentence).closest('a');
    const badge = (sentence: string) => entry(sentence)?.querySelector('.nav-badge');
    expect(screen.getByRole('link', { name: /Diagnostics 3 warnings/ })).toBeTruthy();
    expect(badge('3 warnings')).toHaveTextContent('3');
    expect(entry('3 warnings')?.getAttribute('href')).toBe(href('/diagnostics'));
    expect(entry('2 running')?.getAttribute('href')).toBe(href('/tasks'));
    // Where the waiting proposals are applied, not where past ones are listed.
    expect(entry('5 awaiting review')?.getAttribute('href')).toBe(href('/simulation'));
    expect(entry('1 failed')?.getAttribute('href')).toBe(href('/move-log'));

    // Amber asks for something, red says something broke, the rest is just a
    // number: four amber badges would say everything is urgent.
    expect(badge('3 warnings')?.className).toContain('is-warning');
    expect(badge('1 failed')?.className).toContain('is-critical');
    expect(badge('2 running')?.className).not.toContain('is-');
    unmount();

    show();
    expect(screen.queryByText(/warnings/)).toBeNull();
    expect(screen.queryByText(/running/)).toBeNull();
  });
});
