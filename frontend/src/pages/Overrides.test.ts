import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { media, paginated } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import type { Category, OverrideEntry } from '../api/types';
import Overrides from './Overrides.svelte';
import { answerConfirmation } from '../test/confirm';

/**
 * An override short-circuits the whole rule engine for one item, so the screen
 * that creates them is the one place a user pins a decision by hand. What it
 * must not do is create one against nothing, or delete one without asking.
 */

const STRINGS = {
  Overrides: 'Overrides',
  NewOverride: 'New override',
  NoOverrides: 'No override yet',
  Delete: 'Delete',
  Search: 'Search',
  SelectItem: 'Select',
  SearchLibrary: 'Search the library by title',
  PinMediaTitle: 'Pin an item to a category',
  ConfirmDeleteOverride: 'Remove the override on “{title}”?',
  OverrideRemoved: 'Override removed',
  CreateOverride: 'Pin it',
  ForceCategoryFor: 'Force category for “{title}”',
  None: '—',
};

const categories = [
  {
    id: 'c1',
    name: 'anime',
    description: null,
    is_default: false,
    display_order: 1,
    created_at: '',
    rule_count: 0,
    root_folder_count: 1,
  },
  {
    id: 'c2',
    name: 'kids',
    description: null,
    is_default: false,
    display_order: 2,
    created_at: '',
    rule_count: 0,
    root_folder_count: 1,
  },
] as Category[];

function override(over: Partial<OverrideEntry> = {}): OverrideEntry {
  return {
    id: 'o1',
    media_id: 'm1',
    target_category: 'anime',
    reason: null,
    locked: false,
    created_at: '2026-08-27 10:00:00',
    media_title: 'Akira',
    media_type: 'movie',
    instance_name: 'Radarr',
    ...over,
  };
}

function show(overrides: OverrideEntry[]) {
  vi.spyOn(api, 'getOverrides').mockResolvedValue(overrides);
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  return renderWithI18n(Overrides, { strings: STRINGS });
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Overrides', () => {
  it('says there are none rather than showing an empty table', async () => {
    show([]);

    expect(await screen.findByText('No override yet')).toBeTruthy();
  });

  /**
   * Every row carries the same destructive button. Announced as "Delete" and
   * nothing else, a screen reader gives the user N identical buttons and no way
   * to tell which override each one removes.
   */
  it('names each delete button after the item it would unpin', async () => {
    show([override({ media_title: 'Akira' }), override({ id: 'o2', media_title: 'Totoro' })]);

    expect(await screen.findByRole('button', { name: 'Delete — Akira' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Delete — Totoro' })).toBeTruthy();
  });

  it('asks before removing, naming what would be removed', async () => {
    const remove = vi.spyOn(api, 'deleteOverride');
    show([override()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete — Akira' }));

    // Cancelled, and the question named the subject: "are you sure?" over a
    // table of twelve rows tells the user nothing about which one.
    expect(await answerConfirmation(null)).toBe('Remove the override on “Akira”?');
    expect(remove).not.toHaveBeenCalled();
  });

  /**
   * The banner stays until something replaces it. Left standing, the first
   * removal reads as the outcome of the second, printed above the reason that
   * one was refused.
   */
  it('takes the previous success off screen when the next removal is refused', async () => {
    vi.spyOn(api, 'deleteOverride')
      .mockResolvedValueOnce(undefined as never)
      .mockRejectedValueOnce(new ApiError('The override is locked', 409, 'conflict'));
    show([override({ media_title: 'Akira' }), override({ id: 'o2', media_title: 'Totoro' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete — Akira' }));
    await answerConfirmation();
    expect(await screen.findByText('Override removed')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete — Totoro' }));
    await answerConfirmation();

    expect(await screen.findByText('The override is locked')).toBeTruthy();
    expect(screen.queryByText('Override removed')).toBeNull();
  });

  it('removes it once the question is answered', async () => {
    const remove = vi.spyOn(api, 'deleteOverride').mockResolvedValue(undefined as never);
    show([override()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete — Akira' }));
    await answerConfirmation();

    await waitFor(() => expect(remove).toHaveBeenCalledWith('o1'));
    expect(await screen.findByText('Override removed')).toBeTruthy();
  });

  /**
   * The pin button is disabled until an item is chosen. Without that, saving
   * sends `media_id: undefined` and the backend answers with a validation error
   * for something the user never filled in — they searched and forgot to click.
   */
  it('will not pin until an item has actually been chosen', async () => {
    const create = vi.spyOn(api, 'createOverride');
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New override' }));
    const pin = await screen.findByRole('button', { name: 'Pin it' });

    expect((pin as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(pin);
    expect(create).not.toHaveBeenCalled();
  });

  /**
   * The result row was a `<tr onclick>`: no tab stop, no role, no key. Creating
   * an exception was impossible from the keyboard or a screen reader — and
   * Svelte warns about that on a `<div>`, not on a `<tr>`.
   */
  it('lets the keyboard pick an item, through a control that says what it picks', async () => {
    const picked = media({ id: 'm7', title: 'Perfect Blue' });
    vi.spyOn(api, 'getMedia').mockResolvedValue(paginated([picked]));
    show([]);
    const user = userEvent.setup();

    await fireEvent.click(await screen.findByRole('button', { name: 'New override' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);

    const pick = await screen.findByRole('button', { name: 'Select — Perfect Blue' });
    expect(pick).toHaveAttribute('aria-pressed', 'false');
    pick.focus();
    await user.keyboard('{Enter}');

    expect(pick).toHaveAttribute('aria-pressed', 'true');
    expect(await screen.findByLabelText('Force category for “Perfect Blue”')).toBeTruthy();
  });

  it('shows a refused pin inside the dialog rather than behind it', async () => {
    const picked = media({ id: 'm7', title: 'Perfect Blue' });
    vi.spyOn(api, 'getMedia').mockResolvedValue(paginated([picked]));
    vi.spyOn(api, 'createOverride').mockRejectedValue(
      new ApiError('This item is already pinned', 409, 'conflict'),
    );
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New override' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);
    await fireEvent.click(await screen.findByText('Perfect Blue'));
    await fireEvent.click(await screen.findByRole('button', { name: 'Pin it' }));

    const dialog = await screen.findByRole('dialog');
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('already pinned');
  });

  it('pins the item that was picked, in the category that was picked', async () => {
    const picked = media({ id: 'm7', title: 'Perfect Blue' });
    vi.spyOn(api, 'getMedia').mockResolvedValue(paginated([picked]));
    const create = vi.spyOn(api, 'createOverride').mockResolvedValue(undefined as never);
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New override' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);

    await fireEvent.click(await screen.findByText('Perfect Blue'));
    await fireEvent.click(await screen.findByRole('button', { name: 'Pin it' }));

    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(nthCall(create)[0]).toMatchObject({ media_id: 'm7', target_category: 'anime' });
  });
});
