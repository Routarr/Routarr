import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { SCREENS } from '../lib/routes';
import { HELP, TERMS } from '../lib/help';
import { renderWithI18n } from '../test/render';
import { nthCall } from '../test/spy';
import { media as film, explainedMedia } from '../test/fixtures';
import type { MediaListItem } from '../api/types';
import { api } from '../api/client';
import { router } from '../lib/router.svelte';
import CommandPalette from './CommandPalette.svelte';

/**
 * One field, from anywhere, for the question this product exists to answer.
 *
 * "Why did Routarr put this film there?" otherwise takes four steps from any
 * screen: open the library, type, search, then find the row and press its
 * button. What is asserted here is that it takes one, and that the palette
 * carries nothing that writes to a library.
 */

const STRINGS = {
  CommandPalette: 'Quick search',
  CommandPalettePlaceholder: 'A title, or a screen…',
  CommandPaletteGoTo: 'Go to',
  CommandPaletteEmpty: 'Nothing for "{query}".',
  CommandPaletteHint: 'The library is searched by title.',
  CommandPaletteResults: 'Results: {count}',
  Loading: 'Loading…',
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
  RootFolders: 'Categories and folders',
  Applications: 'Applications',
  ApiReference: 'API reference',
  Settings: 'Settings',
  HintRules: 'What goes where',
  Dismiss: 'Dismiss',
  PinAsRuleTest: 'Pin as a rule test',
};

const media = (over: Partial<MediaListItem> = {}) =>
  film({ id: 'm-1', title: 'Spirited Away', year: 2001, ...over });

const show = (onClose = () => {}, onHelp = vi.fn()) =>
  renderWithI18n(CommandPalette, { props: { onClose, onHelp }, strings: STRINGS });

afterEach(() => vi.restoreAllMocks());

describe('CommandPalette', () => {
  /** "History" the screen and "History" the film are told apart by their group. */
  it('names each run of results by its heading', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [media({ title: 'History' })],
      pagination: { page: 1, per_page: 5, total: 1, total_pages: 1 },
    });
    show();

    await userEvent.type(screen.getByRole('combobox'), 'history');

    const library = await screen.findByRole('group', { name: 'Library' });
    expect(library).toHaveTextContent('History');
    expect(screen.getByRole('group', { name: 'Go to' })).toHaveTextContent('History');
  });

  it('offers every destination before anything is typed', async () => {
    show();

    const options = await screen.findAllByRole('option');
    // Every screen, and nothing else: an empty field proposes where to go
    // rather than an empty box.
    expect(options).toHaveLength(SCREENS.length);
    expect(options[0]).toHaveTextContent('Dashboard');
  });

  /** A few words beside each name, so the list of screens reads as what each is for. */
  it('describes each destination beside its name', async () => {
    show();

    const rules = (await screen.findAllByRole('option')).find((option) =>
      option.textContent?.includes('Rules'),
    );
    expect(rules).toHaveTextContent('What goes where');
  });

  /** Opening it is not a promise to go somewhere. */
  it('closes without a choice, from its own button or a click beside it', async () => {
    const onClose = vi.fn();
    show(onClose);

    await fireEvent.click(await screen.findByRole('button', { name: 'Dismiss' }));
    expect(onClose).toHaveBeenCalledTimes(1);

    await userEvent.click(screen.getByRole('dialog'));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('narrows the destinations on what is typed', async () => {
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'diag');
    const destinations = within(screen.getByRole('group', { name: 'Go to' }));
    expect(destinations.getAllByRole('option')).toHaveLength(1);
    expect(destinations.getByRole('option')).toHaveTextContent('Diagnostics');
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
    expect(getMedia).toHaveBeenCalledWith(
      { search: 'spirited', per_page: 5 },
      expect.any(AbortSignal),
    );
    // What tells one film from another with the same name.
    expect(row.nextElementSibling).toHaveTextContent('2001 · Radarr · anime');
  });

  /** A search the typing has moved past is abandoned, not left to run on the server. */
  it('abandons the search a new term replaces', async () => {
    const getMedia = vi.spyOn(api, 'getMedia').mockReturnValue(new Promise(() => {}));
    show();
    await screen.findAllByRole('option');
    const field = screen.getByRole('combobox');

    await userEvent.type(field, 'akira');
    await waitFor(() => expect(getMedia).toHaveBeenCalledTimes(1));
    const first = nthCall(getMedia)[1];
    expect(first?.aborted).toBe(false);

    await userEvent.type(field, ' 2');
    await waitFor(() => expect(first?.aborted).toBe(true));
  });

  it('says nothing rather than showing an empty box', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [],
      pagination: { page: 1, per_page: 5, total: 0 },
    } as never);
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'zzzz');
    expect(await screen.findByText(/Nothing for/)).toHaveTextContent('Nothing for "zzzz".');
  });

  /** "Nothing for" before the library answered is a claim it has not made yet. */
  it('says it is searching until the library answers, not that nothing matched', async () => {
    let answer = () => {};
    vi.spyOn(api, 'getMedia').mockReturnValue(
      new Promise((resolve) => {
        answer = () =>
          resolve({ data: [], pagination: { page: 1, per_page: 5, total: 0 } } as never);
      }),
    );
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'zzzz');

    expect(await screen.findByText('Loading…')).toBeInTheDocument();
    expect(screen.queryByText(/Nothing for/)).toBeNull();
    answer();
    expect(await screen.findByText(/Nothing for/)).toBeInTheDocument();
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
    // field has to keep. An `option` must not hold interactive content
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

    expect(router.path).toBe('/diagnostics');
    expect(onClose).toHaveBeenCalled();
  });

  /**
   * The answer arrives here rather than at the end of a navigation: neither
   * the explanation nor the rule editor is addressable by URL, so sending
   * someone to `/library` would be step one of the four steps this replaces.
   */
  it('answers "why is this here" without leaving the page', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [media()],
      pagination: { page: 1, per_page: 5, total: 1 },
    } as never);
    const explain = vi.spyOn(api, 'explainMedia').mockResolvedValue({
      media: explainedMedia({
        id: 'm-1',
        title: 'Spirited Away',
        year: 2001,
        current_path: '/movies/standard/Spirited Away',
      }),
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

    await vi.waitFor(() => expect(explain).toHaveBeenCalledWith('m-1', expect.any(AbortSignal)));
    // The router never moved: the question was answered where it was asked.
    expect(router.path).not.toBe('/library');
  });

  /** The panel arrives a moment later, and the title asked about says so meanwhile. */
  it('marks the title being explained as busy', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [media()],
      pagination: { page: 1, per_page: 5, total: 1 },
    } as never);
    vi.spyOn(api, 'explainMedia').mockReturnValue(new Promise(() => {}));
    show();
    await screen.findAllByRole('option');

    await userEvent.type(screen.getByRole('combobox'), 'spirited');
    await fireEvent.click(await screen.findByText('Spirited Away'));

    expect(screen.getByRole('option', { name: /Spirited Away/ })).toHaveAttribute(
      'aria-busy',
      'true',
    );
  });

  /**
   * Only the library needs the network. Rendered in place of the list, the
   * failure would take the destinations with it, and an error never cleared
   * would leave the field able to do nothing at all until it is closed and
   * reopened.
   */
  it('keeps the destinations when the library cannot be reached', async () => {
    vi.spyOn(api, 'getMedia').mockRejectedValue(new Error('unreachable'));
    show();
    await screen.findAllByRole('option');

    // A term that names a screen as well as a film: what must survive the
    // failure is the half that never needed the network.
    await userEvent.type(screen.getByRole('combobox'), 'logs');
    await screen.findByRole('alert');
    expect(
      within(screen.getByRole('group', { name: 'Go to' })).getByRole('option'),
    ).toHaveTextContent('Logs');

    // And the failure goes when the question does.
    await userEvent.clear(screen.getByRole('combobox'));
    await vi.waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
  });

  /**
   * `aria-expanded` describes what is rendered. Hard-coded true, the field
   * would claim to control a listbox that the empty branch removes, which is a
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
   * guardrails that live on the screen owning them. The explanation it opens is
   * the library screen's own panel, and its one write, pinning the case as a
   * rule test, stays inside Routarr.
   */
  it('offers nothing that writes to a library, in its list or in the explanation it opens', async () => {
    const WRITES = /apply|revert|delete|remove|sync|move|override/i;
    const actions = (scope: HTMLElement) =>
      [...scope.querySelectorAll('button')].map(
        (button) => button.getAttribute('aria-label') ?? button.textContent?.trim() ?? '',
      );
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [media()],
      pagination: { page: 1, per_page: 5, total: 1, total_pages: 1 },
    });
    vi.spyOn(api, 'explainMedia').mockResolvedValue({
      media: explainedMedia({
        id: 'm-1',
        title: 'Spirited Away',
        year: 2001,
        current_path: '/movies/standard/Spirited Away',
      }),
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

    await userEvent.type(screen.getByRole('combobox'), 'spirited');
    const palette = screen.getByRole('dialog', { name: 'Quick search' });
    expect(actions(palette).filter((name) => WRITES.test(name))).toEqual([]);
    await fireEvent.click(await screen.findByRole('option', { name: /Spirited Away/ }));

    const panel = await screen.findByRole('dialog', { name: 'Spirited Away' });
    expect(actions(panel)).toContain('Pin as a rule test');
    expect(actions(panel).filter((name) => WRITES.test(name))).toEqual([]);
  });

  /** The help answers too, and opens on the screen it found. */
  it('offers what the help says, and opens it on that screen', async () => {
    vi.spyOn(api, 'getMedia').mockResolvedValue({
      data: [],
      pagination: { page: 1, per_page: 5, total: 0, total_pages: 0 },
    });
    const term = Object.keys(TERMS)[0]!;
    const onHelp = vi.fn();
    renderWithI18n(CommandPalette, {
      props: { onClose: () => {}, onHelp },
      strings: { ...STRINGS, Help: 'Help', [TERMS[term]!.name]: 'Quokka' },
    });

    await userEvent.type(screen.getByRole('combobox', { name: 'Quick search' }), 'quokka');
    const group = await screen.findByRole('group', { name: 'Help' });
    await userEvent.click(within(group).getByRole('option', { name: /Quokka/ }));

    expect(onHelp).toHaveBeenCalledWith(SCREENS.find((path) => HELP[path]!.terms.includes(term)));
  });
});
