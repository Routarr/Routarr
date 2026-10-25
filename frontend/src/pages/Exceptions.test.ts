import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { explainedMedia, explanation, media, paginated } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import type { Category, OverrideEntry } from '../api/types';
import Exceptions from './Exceptions.svelte';
import { publishAccess } from '../lib/access.svelte';
import { answerConfirmation } from '../test/confirm';
import { dropFocus } from '../test/focus';

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
    created_at: '2026-08-27 10:00:00',
    subject: null,
    media_title: 'Akira',
    media_type: 'movie',
    instance_name: 'Radarr',
    ...over,
  };
}

function show(overrides: OverrideEntry[]) {
  vi.spyOn(api, 'getExceptions').mockResolvedValue(paginated(overrides));
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  return renderWithI18n(Exceptions);
}

afterEach(() => vi.restoreAllMocks());

describe('Exceptions', () => {
  it('says there are none rather than showing an empty table', async () => {
    show([]);

    expect(
      await screen.findByText('No exception. Every media item is classified by the rules.'),
    ).toBeTruthy();
  });

  it('names who set a pin under the day it was set', async () => {
    show([override({ subject: 'request-bot' })]);

    const row = (await screen.findByText('Akira')).closest('tr') as HTMLElement;
    expect(within(row).getByText('request-bot').closest('td')).toHaveTextContent(
      /Performed by\s*request-bot/,
    );
  });

  /** An exception the explanation replaces changes the list behind it. */
  it('explains the title an exception holds, and reads the list again once it changed', async () => {
    vi.spyOn(api, 'getSources').mockResolvedValue({ providers: [], order: [] });
    vi.spyOn(api, 'getConditionCatalog').mockResolvedValue({
      match_modes: ['all', 'any'],
      conditions: [],
      score_scales: [],
    });
    const explain = vi.spyOn(api, 'explainTitle').mockResolvedValue(
      explanation({
        media: explainedMedia({ id: 'm1', title: 'Akira' }),
        current_category: 'standard',
        override_category: 'anime',
      }),
    );
    vi.spyOn(api, 'keepTitle').mockResolvedValue({} as never);
    show([override()]);

    await userEvent.click(await screen.findByRole('button', { name: 'Why? – Akira' }));
    expect(explain).toHaveBeenCalledWith('m1', expect.any(AbortSignal));
    const dialog = await screen.findByRole('dialog');
    const loads = vi.mocked(api.getExceptions).mock.calls.length;
    await userEvent.click(within(dialog).getByRole('button', { name: /^Keep here/ }));
    await answerConfirmation();

    await waitFor(() =>
      expect(vi.mocked(api.getExceptions).mock.calls.length).toBeGreaterThan(loads),
    );
  });

  it('asks before removing, naming what would be removed', async () => {
    const remove = vi.spyOn(api, 'deleteException');
    show([override()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – Akira' }));

    // Cancelled, and the question named the subject: "are you sure?" over a
    // table of twelve rows tells the user nothing about which one.
    expect(await answerConfirmation(null)).toBe(
      'Remove the exception on "Akira"? The rules take over again.',
    );
    expect(remove).not.toHaveBeenCalled();
  });

  it('takes the previous success off screen when the next removal is refused', async () => {
    vi.spyOn(api, 'deleteException')
      .mockResolvedValueOnce(undefined as never)
      .mockRejectedValueOnce(new ApiError('The override could not be removed', 409, 'conflict'));
    show([override({ media_title: 'Akira' }), override({ id: 'o2', media_title: 'Totoro' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – Akira' }));
    await answerConfirmation();
    expect(await screen.findByText('Exception removed')).toBeTruthy();

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – Totoro' }));
    await answerConfirmation();

    expect(await screen.findByText('The override could not be removed')).toBeTruthy();
    expect(screen.queryByText('Exception removed')).toBeNull();
  });

  it('removes it once the question is answered', async () => {
    const remove = vi.spyOn(api, 'deleteException').mockResolvedValue(undefined as never);
    show([override()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – Akira' }));
    await answerConfirmation();

    await waitFor(() => expect(remove).toHaveBeenCalledWith('o1'));
    expect(await screen.findByText('Exception removed')).toBeTruthy();
  });

  /** Sent without a title, a pin comes back refused for a field the reader never saw. */
  it('will not pin until an item has actually been chosen', async () => {
    const create = vi.spyOn(api, 'createException');
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const pin = await screen.findByRole('button', { name: 'Create exception' });

    expect((pin as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(pin);
    expect(create).not.toHaveBeenCalled();
  });

  it('lets the keyboard pick an item, through a control that says what it picks', async () => {
    const picked = media({ id: 'm7', title: 'Perfect Blue' });
    vi.spyOn(api, 'getLibrary').mockResolvedValue(paginated([picked]));
    show([]);
    const user = userEvent.setup();

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);

    const pick = await screen.findByRole('button', { name: 'Select – Perfect Blue' });
    expect(pick).toHaveAttribute('aria-pressed', 'false');
    pick.focus();
    await user.keyboard('{Enter}');

    expect(pick).toHaveAttribute('aria-pressed', 'true');
    expect(await screen.findByLabelText('Force category for "Perfect Blue"')).toBeTruthy();
  });

  /** An empty library is told to sync, a search that missed in a full one is not. */
  it.each([
    [0, 'No media matches. Sync an instance from the Instances screen to populate this list.'],
    [12, 'No title matches this search or these filters.'],
  ])('says a search found nothing in a library of %i titles', async (held, said) => {
    vi.spyOn(api, 'getLibrary').mockImplementation((params) =>
      Promise.resolve(params?.search ? paginated([]) : paginated([], { total: held })),
    );
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const search = await screen.findByLabelText('Search the library by title');
    expect(screen.queryByText(said)).toBeNull();
    await fireEvent.input(search, { target: { value: 'nothing' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);

    expect(await screen.findByText(said)).toBeTruthy();
  });

  it('says how many it shows of how many matched, under named columns', async () => {
    const found = Array.from({ length: 15 }, (_, i) => media({ id: `m${i}`, title: `Star ${i}` }));
    vi.spyOn(api, 'getLibrary').mockResolvedValue(paginated(found, { total: 40 }));
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const search = await screen.findByLabelText('Search the library by title');
    const region = within(screen.getByRole('dialog')).getByRole('status');
    await fireEvent.input(search, { target: { value: 'star' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);

    await waitFor(() =>
      expect(region).toHaveTextContent(
        'Showing the first 15 of 40. Refine the search to find the others.',
      ),
    );
    const dialog = screen.getByRole('dialog');
    expect(
      within(dialog)
        .getAllByRole('columnheader')
        .map((header) => header.textContent),
    ).toEqual(['Title', 'Instance', 'Current root folder']);
  });

  it('shows a refused pin inside the dialog rather than behind it', async () => {
    const picked = media({ id: 'm7', title: 'Perfect Blue' });
    vi.spyOn(api, 'getLibrary').mockResolvedValue(paginated([picked]));
    vi.spyOn(api, 'createException').mockRejectedValue(
      new ApiError('This item is already pinned', 409, 'conflict'),
    );
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);
    await fireEvent.click(await screen.findByText('Perfect Blue'));
    await fireEvent.click(await screen.findByRole('button', { name: 'Create exception' }));

    const dialog = await screen.findByRole('dialog');
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('already pinned');
  });

  /** The button turns disabled while it writes, which drops its focus out of the dialog. */
  it('gives the focus back to Create exception once a pin is refused', async () => {
    vi.spyOn(api, 'getLibrary').mockResolvedValue(
      paginated([media({ id: 'm7', title: 'Perfect Blue' })]),
    );
    let refuse = () => {};
    vi.spyOn(api, 'createException').mockReturnValue(
      new Promise((_, reject) => (refuse = () => reject(new Error('Refused')))),
    );
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);
    await fireEvent.click(await screen.findByText('Perfect Blue'));
    const pin = await screen.findByRole('button', { name: 'Create exception' });
    await userEvent.click(pin);
    await waitFor(() => expect(pin).toBeDisabled());
    dropFocus();
    refuse();

    await waitFor(() => expect(document.activeElement).toBe(pin));
  });

  it('pins once however often Create exception is pressed', async () => {
    vi.spyOn(api, 'getLibrary').mockResolvedValue(
      paginated([media({ id: 'm7', title: 'Perfect Blue' })]),
    );
    const create = vi.spyOn(api, 'createException').mockReturnValue(new Promise(() => {}));
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);
    await fireEvent.click(await screen.findByText('Perfect Blue'));
    const pin = await screen.findByRole('button', { name: 'Create exception' });
    await fireEvent.click(pin);
    await fireEvent.click(pin);

    expect(create).toHaveBeenCalledTimes(1);
  });

  it('pins the item that was picked, in the category that was picked', async () => {
    const picked = media({ id: 'm7', title: 'Perfect Blue' });
    vi.spyOn(api, 'getLibrary').mockResolvedValue(paginated([picked]));
    const create = vi.spyOn(api, 'createException').mockResolvedValue(undefined as never);
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));
    const search = await screen.findByLabelText('Search the library by title');
    await fireEvent.input(search, { target: { value: 'perfect' } });
    await fireEvent.submit(search.closest('form') as HTMLFormElement);

    await fireEvent.click(await screen.findByText('Perfect Blue'));
    // Not the first category, which is what the dialog would send unasked.
    await userEvent.selectOptions(
      await screen.findByLabelText('Force category for "Perfect Blue"'),
      'kids',
    );
    await fireEvent.click(await screen.findByRole('button', { name: 'Create exception' }));

    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(nthCall(create)[0]).toMatchObject({ media_id: 'm7', target_category: 'kids' });
  });

  it('opens the pin dialog on its search field', async () => {
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New exception' }));

    expect(document.activeElement).toBe(
      await screen.findByRole('searchbox', { name: 'Search the library by title' }),
    );
  });
});

it('hands the focus to the exception that took the place of the removed one', async () => {
  const next = override({ id: 'o2', media_title: 'Perfect Blue' });
  show([override(), next]);
  vi.spyOn(api, 'deleteException').mockResolvedValue(undefined as never);

  await fireEvent.click(await screen.findByRole('button', { name: 'Delete – Akira' }));
  vi.spyOn(api, 'getExceptions').mockResolvedValue(paginated([next]));
  await answerConfirmation();

  await waitFor(() =>
    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: 'Delete – Perfect Blue' }),
    ),
  );
});

/** The exceptions come a page at a time: a script may pin a whole library. */
describe('more exceptions than a page holds', () => {
  it('asks the server for the next page', async () => {
    const list = vi
      .spyOn(api, 'getExceptions')
      .mockResolvedValue(paginated([override()], { total: 51, total_pages: 2 }));
    vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
    renderWithI18n(Exceptions);

    await userEvent.click(await screen.findByRole('button', { name: 'Next' }));

    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith({ page: 2, per_page: 50 }, expect.any(AbortSignal)),
    );
  });

  it('steps back a page once the last exception of the last one is removed', async () => {
    const list = vi
      .spyOn(api, 'getExceptions')
      .mockResolvedValueOnce(paginated([override()], { total: 51, total_pages: 2 }))
      .mockResolvedValueOnce(paginated([override()], { page: 2, total: 51, total_pages: 2 }))
      .mockResolvedValueOnce(paginated([], { page: 2, total: 50, total_pages: 1 }))
      .mockResolvedValue(paginated([override({ id: 'o3', media_title: 'Paprika' })]));
    vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
    vi.spyOn(api, 'deleteException').mockResolvedValue(undefined as never);
    renderWithI18n(Exceptions);
    await userEvent.click(await screen.findByRole('button', { name: 'Next' }));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));

    await userEvent.click(await screen.findByRole('button', { name: 'Delete – Akira' }));
    await answerConfirmation();

    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith({ page: 1, per_page: 50 }, expect.any(AbortSignal)),
    );
  });
});

/** A reader is refused both the pin and its removal: the list is what they may read. */
describe('for a reader', () => {
  const offered = () =>
    ['New exception', 'Delete – Akira'].filter((name) => screen.queryByRole('button', { name }));

  it('offers the owner the new exception and the delete, and a reader neither', async () => {
    const { unmount } = show([override()]);
    await screen.findByText('Akira');
    expect(offered()).toEqual(['New exception', 'Delete – Akira']);
    unmount();

    publishAccess(true);
    show([override({ reason: 'Kept by hand' })]);

    expect(await screen.findByText('Kept by hand')).toBeInTheDocument();
    expect(offered()).toEqual([]);
  });
});
