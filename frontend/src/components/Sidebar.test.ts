import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { href, navigate } from '../lib/router.svelte';
import { withBase } from '../test/base';
import Sidebar from './Sidebar.svelte';

/**
 * Thirteen destinations in five groups, and below the rail breakpoint the
 * labels are hidden. The name has to reach a screen reader — and a pointer —
 * some other way, or the navigation becomes thirteen unnamed icons.
 */

const STRINGS = {
  Dashboard: 'Dashboard',
  Instances: 'Instances',
  RootFolders: 'Root folders',
  RulesEngine: 'Rules',
  RuleTests: 'Tests',
  MainNavigation: 'Main navigation',
  NavGroupDaily: 'Routing',
  NavGroupReview: 'Review',
  NavGroupSupervision: 'Monitoring',
  NavGroupConfiguration: 'Configuration',
  DiagnosticWarnings: '{count} warnings',
  TasksRunning: '{count} running',
  DecisionsAwaitingReview: '{count} awaiting review',
  FailedMoves: '{count} failed',
  MediaExplorer: 'Media',
  Simulation: 'Simulation',
  AuditHistory: 'History',
  Overrides: 'Overrides',
  Tasks: 'Tasks',
  Logs: 'Logs',
  Diagnostics: 'Diagnostics',
  Settings: 'Settings',
};

const show = (props: Record<string, unknown> = {}) =>
  renderWithI18n(Sidebar, { props, strings: STRINGS });

afterEach(() => {
  withBase(null);
  navigate('/', { replace: true });
  vi.restoreAllMocks();
});

describe('Sidebar', () => {
  it('names every destination, even when the rail hides the label', async () => {
    show();

    const links = await screen.findAllByRole('link');
    // Bump this when a destination is added. It is not the claim — the loop
    // below is — but without it the loop passes over an empty list and asserts
    // nothing at all.
    expect(links).toHaveLength(13);
    for (const link of links) {
      expect(link.getAttribute('title')).toBeTruthy();
    }
  });

  /**
   * Marking the active entry by colour alone says nothing to a screen reader
   * and disappears for anyone who cannot separate the two greys.
   */
  it('announces which entry is current, rather than only colouring it', async () => {
    navigate('/rules', { replace: true });
    show();

    const current = await screen.findByRole('link', { current: 'page' });
    expect(current.textContent).toContain('Rules');
  });

  /**
   * The dashboard is matched exactly, without which every path starts with "/"
   * and it is highlighted on all twelve screens.
   */
  it('does not mark the dashboard current from another screen', async () => {
    navigate('/logs', { replace: true });
    show();

    const current = await screen.findByRole('link', { current: 'page' });
    expect(current.textContent).toContain('Logs');
  });

  it('closes the drawer when a destination is chosen', async () => {
    const onNavigate = vi.fn();
    show({ open: true, onNavigate });

    await fireEvent.click(await screen.findByRole('link', { name: /Logs/ }));

    expect(onNavigate).toHaveBeenCalledTimes(1);
  });

  /// The order is the argument: the screen the product exists for comes first,
  /// and what is configured once at install goes last.
  it('opens on routing and ends on configuration', () => {
    show();

    const links = screen.getAllByRole('link').map((link) => link.getAttribute('href'));
    expect(links.slice(0, 4)).toEqual(['/', '/rules', '/rules/tests', '/simulation']);
    expect(links.slice(-3)).toEqual(['/instances', '/root-folders', '/settings']);
  });

  /// A group of two or three is read at a glance where a list of thirteen is
  /// scanned every time — but only if a reader is told the groups exist.
  it('names every group, and the navigation itself', () => {
    show();

    expect(screen.getByRole('navigation', { name: 'Main navigation' })).toBeInTheDocument();
    for (const group of ['Routing', 'Review', 'Monitoring', 'Configuration']) {
      expect(screen.getByRole('list', { name: group })).toBeInTheDocument();
    }
  });

  /// In the rail the badge is the only thing that says something is waiting,
  /// and a zero must not draw the eye to nothing.
  /**
   * A `<base href>` applies to relative URLs only. Left root-absolute, the
   * left click worked because the router prefixed the mount point, and a
   * middle-click, a ctrl-click or a copied link went to the proxy's root.
   */
  it('prefixes every destination with the mount point a reverse proxy adds', () => {
    withBase('/routarr/');
    show({ counts: { jobs: 1, decisions: 0, failed: 0, warnings: 0 } });

    const links = screen.getAllByRole('link').map((a) => a.getAttribute('href'));
    expect(links.length).toBeGreaterThanOrEqual(13);
    for (const link of links) expect(link).toMatch(/^\/routarr\//);
    expect(links).toContain('/routarr/rules');
    expect(links).toContain('/routarr/');
  });

  it('carries each count onto the entry that answers it, and nothing when there is none', () => {
    const { unmount } = show({
      counts: { jobs: 2, decisions: 5, failed: 1, warnings: 3 },
    });

    // The sentence the top bar used to state, now part of the link's name —
    // an `aria-label` on a generic span reached nobody reliably.
    const entry = (sentence: string) => screen.getByText(sentence).closest('a');
    const badge = (sentence: string) => entry(sentence)?.querySelector('.nav-badge');
    expect(screen.getByRole('link', { name: /Diagnostics 3 warnings/ })).toBeTruthy();
    expect(badge('3 warnings')).toHaveTextContent('3');
    expect(entry('3 warnings')?.getAttribute('href')).toBe(href('/health'));
    expect(entry('2 running')?.getAttribute('href')).toBe(href('/jobs'));
    expect(entry('5 awaiting review')?.getAttribute('href')).toBe(href('/history'));
    expect(entry('1 failed')?.getAttribute('href')).toBe(href('/logs'));

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
