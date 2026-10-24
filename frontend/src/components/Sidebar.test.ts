import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { SCREENS } from '../lib/routes';
import { renderWithI18n } from '../test/render';
import { href, navigate } from '../lib/router.svelte';
import { withBase } from '../test/base';
import { click } from '../test/links';
import { publishAccess } from '../lib/access.svelte';
import Sidebar from './Sidebar.svelte';

const show = (props: Record<string, unknown> = {}) => renderWithI18n(Sidebar, { props });

afterEach(() => {
  withBase(null);
  vi.restoreAllMocks();
});

describe('Sidebar', () => {
  it('opens the About dialog from the version', async () => {
    const onAbout = vi.fn();
    show({ version: '0.2.0', onAbout });

    const version = screen.getByRole('button', { name: /v0\.2\.0/ });
    expect(version).toHaveAttribute('title', 'About Routarr');
    await fireEvent.click(version);

    expect(onAbout).toHaveBeenCalledOnce();
  });

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

  it('names every group, and the navigation itself', () => {
    show();

    expect(screen.getByRole('navigation', { name: 'Main navigation' })).toBeInTheDocument();
    for (const group of ['Routing', 'Review', 'Monitoring', 'Configuration', 'Integrations']) {
      expect(screen.getByRole('list', { name: group })).toBeInTheDocument();
    }
  });

  it('prefixes every destination with the mount point a reverse proxy adds', () => {
    withBase('/routarr/');
    show({ counts: { jobs: 1, decisions: 0, failed: 0, warnings: 0 } });

    const links = screen.getAllByRole('link').map((a) => a.getAttribute('href'));
    expect(links.length).toBeGreaterThanOrEqual(SCREENS.length);
    for (const link of links) expect(link).toMatch(/^\/routarr\//);
    expect(links).toContain('/routarr/rules');
    expect(links).toContain('/routarr/');
  });

  it('carries each count onto the entry that answers it, and nothing when there is none', () => {
    const { unmount } = show({
      counts: { jobs: 2, decisions: 5, failed: 1, warnings: 3 },
    });

    const entry = (sentence: string) => screen.getByText(sentence).closest('a');
    const badge = (sentence: string) => entry(sentence)?.querySelector('.nav-badge');
    expect(screen.getByRole('link', { name: /Diagnostics Warnings: 3/ })).toBeTruthy();
    expect(badge('Warnings: 3')).toHaveTextContent('3');
    expect(entry('Warnings: 3')?.getAttribute('href')).toBe(href('/diagnostics'));
    expect(entry('Running tasks: 2')?.getAttribute('href')).toBe(href('/tasks'));
    // Where the waiting proposals are applied, not where past ones are listed.
    expect(entry('Decisions awaiting review: 5')?.getAttribute('href')).toBe(href('/simulation'));
    expect(entry('Failed moves: 1')?.getAttribute('href')).toBe(href('/move-log'));

    expect(badge('Warnings: 3')?.className).toContain('is-warning');
    expect(badge('Failed moves: 1')?.className).toContain('is-critical');
    expect(badge('Running tasks: 2')?.className).not.toContain('is-');
    unmount();

    show();
    expect(screen.queryByText(/^Warnings: \d+/)).toBeNull();
    expect(screen.queryByText(/^Running tasks: \d+/)).toBeNull();
  });
});

describe('for a reader', () => {
  /** Every read those screens make is refused to a reader, the first included. */
  it('lists no screen the owner alone reads, nor a group left with none', async () => {
    publishAccess(true);
    show();

    const links = await screen.findAllByRole('link');
    const shown = links.map((link) => link.getAttribute('title'));
    for (const name of ['Settings', 'Metadata sources', 'Applications', 'Security log']) {
      expect(shown).not.toContain(name);
    }
    expect(shown).toEqual(expect.arrayContaining(['Rules', 'Instances', 'API reference']));
    expect(links).toHaveLength(SCREENS.length - 4);
    // Each group heading names a list that still holds an entry.
    for (const heading of document.querySelectorAll('.nav-group')) {
      const list = document.querySelector(`[aria-labelledby="${heading.id}"]`);
      expect(list?.querySelectorAll('a').length).toBeGreaterThan(0);
    }
  });
});
