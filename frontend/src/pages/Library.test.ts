import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { category, media, paginated, explainedMedia, explanation } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import type { Explanation, MediaListItem } from '../api/types';
import Library from './Library.svelte';

function show(items: MediaListItem[], pages = 1) {
  // The explanation panel offers the categories a rule test may expect.
  vi.spyOn(api, 'getCategories').mockResolvedValue([category()]);
  vi.spyOn(api, 'getConditionCatalog').mockResolvedValue({
    match_modes: ['all', 'any'],
    conditions: [],
    score_scales: [],
  });
  const getLibrary = vi
    .spyOn(api, 'getLibrary')
    .mockResolvedValue(paginated(items, { total_pages: pages, total: items.length * pages }));
  renderWithI18n(Library);
  return getLibrary;
}

afterEach(() => vi.restoreAllMocks());

describe('Library', () => {
  it('says nothing matches rather than showing an empty table', async () => {
    show([]);

    expect(
      await screen.findByText(
        'No media matches. Sync an instance from the Instances page to populate this list.',
      ),
    ).toBeTruthy();
  });

  /**
   * An exception short-circuits the engine, so a forced category must not read like a computed
   * one: the lock says a human decided.
   */
  it('distinguishes a category a human forced from one the rules computed', async () => {
    show([
      media({ id: 'm1', title: 'Akira', override_category: 'kids', computed_category: 'anime' }),
      media({ id: 'm2', title: 'Dune', override_category: null, computed_category: 'standard' }),
    ]);

    await screen.findByText('Akira');
    const forced = screen.getByTitle('Manual exception');
    expect(forced.textContent).toContain('kids');
    // The title reaches a mouse alone: a screen reader and a touch reader read the word.
    expect(
      within(screen.getByRole('row', { name: /Akira/ })).getByText('Manual exception'),
    ).toHaveClass('visually-hidden');
    expect(screen.getByText('standard').getAttribute('title')).toBeNull();
  });

  /**
   * "Not evaluated" means no simulation has run, "no category" that no rule matched: a blank
   * cell for either loses the difference.
   */
  it('says an item has not been evaluated rather than leaving the cell blank', async () => {
    show([media({ computed_category: null, override_category: null })]);

    expect(await screen.findByText('not evaluated')).toBeTruthy();
  });

  it('warns about missing metadata', async () => {
    show([media({ has_metadata: false })]);

    const warning = await screen.findByText('missing');
    expect(warning.className).toContain('badge-warning');
  });

  it('gives cached metadata no colour, because having it is not an outcome', async () => {
    show([media({ has_metadata: true })]);

    const badge = await screen.findByText('Metadata', { selector: '.badge' });
    expect(badge.className).not.toContain('badge-warning');
    expect(badge.className).toContain('muted');
  });

  it('says whether a title is a film or a series in words', async () => {
    show([
      media({ id: 'm1', title: 'Akira', media_type: 'movie' }),
      media({ id: 'm2', title: 'Monster', media_type: 'series' }),
    ]);

    expect(await screen.findByRole('row', { name: /Akira/ })).toHaveTextContent('Movies');
    expect(screen.getByRole('row', { name: /Monster/ })).toHaveTextContent('Series');
  });

  it('asks the server to explain the row that was clicked', async () => {
    const explain = vi
      .spyOn(api, 'explainTitle')
      .mockResolvedValue(
        explanation({ media: explainedMedia({ id: 'm7', current_path: '/data/films/Akira' }) }),
      );
    show([media({ id: 'm7', title: 'Akira' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Why? – Akira' }));

    await waitFor(() => expect(explain).toHaveBeenCalledWith('m7', expect.any(AbortSignal)));
  });

  it('marks the row being explained as busy until its answer arrives', async () => {
    let answer = () => {};
    vi.spyOn(api, 'explainTitle').mockReturnValue(
      new Promise((_, reject) => (answer = () => reject(new Error('refused')))),
    );
    show([media({ id: 'm7', title: 'Akira' })]);
    const why = await screen.findByRole('button', { name: 'Why? – Akira' });

    await fireEvent.click(why);
    expect(why).toHaveAttribute('aria-busy', 'true');
    answer();

    await waitFor(() => expect(why).not.toHaveAttribute('aria-busy', 'true'));
  });

  /**
   * The list is paginated server-side. A type filter applied in the browser
   * would filter one page of fifty and present it as the whole answer.
   */
  it('asks the server for the filtered set rather than hiding rows on screen', async () => {
    const getLibrary = show([media()]);
    await screen.findByText('Akira');

    await userEvent.selectOptions(screen.getByLabelText('Filter by type'), 'series');

    await waitFor(() =>
      expect(getLibrary).toHaveBeenCalledWith(
        expect.objectContaining({ media_type: 'series' }),
        expect.any(AbortSignal),
      ),
    );
  });

  it('returns to the first page when the filter changes', async () => {
    const getLibrary = show([media()], 2);
    await screen.findByText('Akira');
    // From page 2, or the assertion below holds on a page that never moved.
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() =>
      expect(getLibrary).toHaveBeenLastCalledWith(
        expect.objectContaining({ page: 2 }),
        expect.any(AbortSignal),
      ),
    );

    await userEvent.selectOptions(screen.getByLabelText('Filter by type'), 'movie');

    // Else a narrower filter keeps a page past its end, which renders as an empty library.
    await waitFor(() =>
      expect(getLibrary).toHaveBeenLastCalledWith(
        expect.objectContaining({ page: 1 }),
        expect.any(AbortSignal),
      ),
    );
  });

  /**
   * Filtered once the typing stops, since each search queries the operator's server, and from
   * the first page, which the narrower list always has.
   */
  it('asks for a typed title once the typing stops, from the first page', async () => {
    const getLibrary = show([media()], 2);
    await screen.findByText('Akira');
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() => expect(getLibrary).toHaveBeenCalledTimes(2));

    await userEvent.type(
      screen.getByRole('searchbox', { name: 'Search the library by title' }),
      'heat',
    );

    await waitFor(() => expect(getLibrary).toHaveBeenCalledTimes(3));
    expect(getLibrary).toHaveBeenLastCalledWith(
      expect.objectContaining({ search: 'heat', page: 1 }),
      expect.any(AbortSignal),
    );
  });

  /** An explanation that cannot be read is not a load of the list to retry. */
  it('reports an explanation it could not read without offering to reload the list', async () => {
    show([media({ title: 'Perfect Blue' })]);
    vi.spyOn(api, 'explainTitle').mockRejectedValue(
      new ApiError('The rules could not be evaluated', 409, 'conflict'),
    );

    await fireEvent.click(await screen.findByRole('button', { name: 'Why? – Perfect Blue' }));

    expect(await screen.findByText('The rules could not be evaluated')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  it('shows the explanation of the title asked about last', async () => {
    show([media({ id: 'm7', title: 'Akira' }), media({ id: 'm8', title: 'Perfect Blue' })]);
    let first: (value: Explanation) => void = () => {};
    const explained = (id: string, title: string): Explanation =>
      explanation({ media: explainedMedia({ id, title }) });
    vi.spyOn(api, 'explainTitle')
      .mockReturnValueOnce(new Promise((resolve) => (first = resolve)))
      .mockResolvedValueOnce(explained('m8', 'Perfect Blue'));

    await fireEvent.click(await screen.findByRole('button', { name: 'Why? – Akira' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Why? – Perfect Blue' }));
    const dialog = await screen.findByRole('dialog');
    first(explained('m7', 'Akira'));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(within(screen.getByRole('dialog')).queryByText(/Akira/)).toBeNull();
    expect(within(dialog).getByText(/Perfect Blue/)).toBeTruthy();
  });

  it('takes a failed explanation off screen once the next one is read', async () => {
    show([media({ id: 'm7', title: 'Perfect Blue' })]);
    // The panel names the sources from the catalogue it reads.
    vi.spyOn(api, 'getSources').mockResolvedValue({ providers: [], order: [] });
    vi.spyOn(api, 'explainTitle')
      .mockRejectedValueOnce(new ApiError('The rules could not be evaluated', 409, 'conflict'))
      .mockResolvedValueOnce(
        explanation({
          media: explainedMedia({ id: 'm7', current_path: '/data/films/Perfect Blue' }),
        }),
      );
    const why = await screen.findByRole('button', { name: 'Why? – Perfect Blue' });
    await fireEvent.click(why);
    expect(await screen.findByText('The rules could not be evaluated')).toBeTruthy();

    await fireEvent.click(why);

    expect(await screen.findByRole('dialog')).toBeTruthy();
    expect(screen.queryByText('The rules could not be evaluated')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });
});
