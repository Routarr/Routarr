import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { api } from '../api/client';
import { router } from '../lib/router.svelte';
import CommandPalette from './CommandPalette.svelte';

/**
 * One field, from anywhere, for the question this product exists to answer.
 *
 * "Why did Routarr put this film there?" took four steps from any screen: open
 * the library, type, search, then find the row and press its button. What is
 * asserted here is that it now takes one — and that the palette carries
 * nothing that writes.
 */

const STRINGS = {
  CommandPalette: 'Quick search',
  CommandPalettePlaceholder: 'A title, or a screen…',
  CommandPaletteGoTo: 'Go to',
  CommandPaletteEmpty: 'Nothing for “{query}”.',
  CommandPaletteHint: 'The library is searched by title.',
  CommandPaletteResults: 'Results: {count}',
  MediaExplorer: 'Library',
  Dashboard: 'Dashboard',
  RulesEngine: 'Rules',
  RuleTests: 'Tests',
  Simulation: 'Simulation',
  AuditHistory: 'History',
  Overrides: 'Overrides',
  Tasks: 'Tasks',
  Logs: 'Logs',
  Diagnostics: 'Diagnostics',
  Instances: 'Instances',
  RootFolders: 'Root folders',
  Settings: 'Settings',
};

function media(over: Record<string, unknown> = {}) {
  return {
    id: 'm-1',
    instance_id: 'i-1',
    instance_name: 'Radarr',
    arr_id: 10,
    media_type: 'movie',
    title: 'Spirited Away',
    year: 2001,
    tmdb_id: 129,
    current_root_folder: '/movies/standard',
    monitored: true,
    has_files: true,
    status: 'released',
    last_synced_at: null,
    computed_category: 'anime',
    override_category: null,
    has_metadata: true,
    ...over,
  };
}

const show = (onClose = () => {}) =>
  renderWithI18n(CommandPalette, { props: { onClose }, strings: STRINGS });

afterEach(() => vi.restoreAllMocks());

describe('CommandPalette', () => {
  it('offers every destination before anything is typed', async () => {
    show();

    const options = await screen.findAllByRole('option');
    // Thirteen screens, and nothing else: an empty field proposes where to go
    // rather than an empty box.
    expect(options).toHaveLength(13);
    expect(options[0]).toHaveTextContent('Dashboard');
  });

  it('narrows the destinations on what is typed', async () => {
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'diag');
    expect(screen.getAllByRole('option')).toHaveLength(1);
    expect(screen.getByRole('option')).toHaveTextContent('Diagnostics');
  });

  /**
   * The library is asked once the typing stops, never on the keystroke: the
   * request goes to somebody's own host, over their own network.
   */
  it('asks the library once, after the typing stops', async () => {
    const getMedia = vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [media()],
      pagination: { page: 1, per_page: 5, total: 1 },
    } as never);
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'spirited');
    expect(getMedia).not.toHaveBeenCalled();

    const row = await screen.findByText('Spirited Away');
    expect(getMedia).toHaveBeenCalledTimes(1);
    expect(getMedia).toHaveBeenCalledWith({ search: 'spirited', per_page: 5 });
    // What tells one film from another with the same name.
    expect(row.nextElementSibling).toHaveTextContent('2001 · Radarr · anime');
  });

  it('says nothing rather than showing an empty box', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [],
      pagination: { page: 1, per_page: 5, total: 0 },
    } as never);
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'zzzz');
    expect(await screen.findByText(/Nothing for/)).toHaveTextContent('Nothing for “zzzz”.');
  });

  /**
   * The combobox pattern: the focus stays in the field so typing can continue
   * while the arrows walk the list, and `aria-activedescendant` is the only
   * thing telling a screen reader where they are.
   */
  it('walks the list without taking the focus out of the field', async () => {
    show();
    const field = await screen.findByRole('combobox');
    const options = screen.getAllByRole('option');

    expect(field.getAttribute('aria-activedescendant')).toBe(options[0]?.id);
    await fireEvent.keyDown(field, { key: 'ArrowDown' });
    expect(field.getAttribute('aria-activedescendant')).toBe(options[1]?.id);
    expect(options[1]?.getAttribute('aria-selected')).toBe('true');

    // Wraps rather than stopping: a list this short is faster walked backwards.
    await fireEvent.keyDown(field, { key: 'ArrowUp' });
    await fireEvent.keyDown(field, { key: 'ArrowUp' });
    expect(field.getAttribute('aria-activedescendant')).toBe(options[options.length - 1]?.id);

    // And nothing in the list is a tab stop, or it would take the focus the
    // field has to keep — an `option` must not hold interactive content
    // either, which is why the row carries its own click.
    for (const option of options) {
      expect(option.querySelector('a, button, input, select, [tabindex]')).toBeNull();
    }
  });

  it('navigates on Enter and closes behind itself', async () => {
    const onClose = vi.fn();
    show(onClose);
    const field = await screen.findByRole('combobox');

    await userEvent.type(field, 'diag');
    await fireEvent.keyDown(field, { key: 'Enter' });

    expect(router.path).toBe('/health');
    expect(onClose).toHaveBeenCalled();
  });

  /**
   * The answer arrives here rather than at the end of a navigation: neither
   * the explanation nor the rule editor is addressable by URL, so sending
   * someone to `/media` would be step one of the four steps this replaces.
   */
  it('answers "why is this here" without leaving the page', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [media()],
      pagination: { page: 1, per_page: 5, total: 1 },
    } as never);
    const explain = vi.spyOn(api, 'explainMedia').mockResolvedValue({
      media: { ...media(), current_path: '/movies/standard/Spirited Away', added_at: null },
      // `null` rather than an empty object: the panel renders a different
      // branch for a title nothing has enriched, and it is the branch that
      // needs no fixture of its own.
      metadata: null,
      override_category: null,
      target_category: 'anime',
      target_root_folder: '/movies/anime',
      action: 'move',
      confidence: 0.7,
      winning_rule: 'Japanese animation',
      rule_traces: [],
    } as never);
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'spirited');
    await screen.findByText('Spirited Away');
    await fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowDown' });
    await fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' });

    await vi.waitFor(() => expect(explain).toHaveBeenCalledWith('m-1'));
    // The router never moved: the question was answered where it was asked.
    expect(router.path).not.toBe('/media');
  });

  /**
   * Only the library needs the network. Rendering the failure in place of the
   * list took the thirteen destinations with it, and since the error was never
   * cleared the field could do nothing at all until it was closed and
   * reopened.
   */
  it('keeps the destinations when the library cannot be reached', async () => {
    vi.spyOn(api, 'getMedia').mockRejectedValue(new Error('unreachable'));
    show();
    await screen.findAllByRole('option');

    // A term that names a screen as well as a film: what must survive the
    // failure is the half that never needed the network.
    await userEvent.type(screen.getByRole('combobox'), 'log');
    await screen.findByRole('alert');
    expect(screen.getByRole('option')).toHaveTextContent('Logs');

    // And the failure goes when the question does.
    await userEvent.clear(screen.getByRole('combobox'));
    await vi.waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
  });

  /**
   * `aria-expanded` describes what is rendered. Hard-coded true, the field
   * claimed to control a listbox that the empty branch had removed, which is a
   * dangling `aria-controls` and an axe failure at WCAG 2.1 A.
   */
  it('stops claiming a list once there is none', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [],
      pagination: { page: 1, per_page: 5, total: 0 },
    } as never);
    show();
    const field = await screen.findByRole('combobox');
    expect(field.getAttribute('aria-expanded')).toBe('true');

    await userEvent.type(field, 'zzzz');
    await screen.findByText(/Nothing for/);
    expect(field.getAttribute('aria-expanded')).toBe('false');
    expect(field.getAttribute('aria-controls')).toBeNull();
    expect(document.getElementById('palette-results')).toBeNull();
  });

  /**
   * A palette exists to be fast, which is the opposite of what a write to
   * somebody's library wants. Applying, reverting and deleting all have
   * guardrails that live on the screen owning them.
   */
  it('carries nothing that writes', async () => {
    show();
    const options = await screen.findAllByRole('option');
    const labels = options.map((option) => option.textContent ?? '');

    expect(labels.some((label) => /apply|revert|delete|sync/i.test(label))).toBe(false);
  });
});
