import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { media, paginated } from '../test/fixtures';
import { api } from '../api/client';
import type { MediaListItem } from '../api/types';
import MediaExplorer from './MediaExplorer.svelte';

/**
 * The library, and the one place a user can ask *why* an item lands where it
 * does. The explanation is the whole explainability contract made visible, so
 * the button that opens it has to be reachable — and named — on every row.
 */

const STRINGS = {
  Next: 'Next',
  MediaExplorer: 'Media',
  SearchLibrary: 'Search the library by title',
  SearchByTitle: 'Search by title…',
  FilterByType: 'Filter by type',
  AllTypes: 'All types',
  Movies: 'Movies',
  Series: 'Series',
  Search: 'Search',
  UnclassifiedOnly: 'Unclassified only',
  NoMediaMatches: 'Nothing matches',
  NotEvaluated: 'not evaluated',
  MetadataCached: 'cached',
  MetadataMissing: 'missing',
  ManualOverride: 'manual override',
  WhyQuestion: 'Why?',
  None: '—',
};

function show(items: MediaListItem[], pages = 1) {
  const getMedia = vi
    .spyOn(api, 'getMedia')
    .mockResolvedValue(paginated(items, { total_pages: pages, total: items.length * pages }));
  renderWithI18n(MediaExplorer, { strings: STRINGS });
  return getMedia;
}

afterEach(() => vi.restoreAllMocks());

describe('Media explorer', () => {
  it('says nothing matches rather than showing an empty table', async () => {
    show([]);

    expect(await screen.findByText('Nothing matches')).toBeTruthy();
  });

  /**
   * An override short-circuits the engine, so the two are different facts about
   * the same row and must not read the same. The lock is what says a human
   * decided this one.
   */
  it('distinguishes a category a human forced from one the rules computed', async () => {
    show([
      media({ id: 'm1', title: 'Akira', override_category: 'kids', computed_category: 'anime' }),
      media({ id: 'm2', title: 'Dune', override_category: null, computed_category: 'standard' }),
    ]);

    await screen.findByText('Akira');
    const forced = screen.getByTitle('manual override');
    expect(forced.textContent).toContain('kids');
    // The computed one carries no such marker.
    expect(screen.getByText('standard').getAttribute('title')).toBeNull();
  });

  /**
   * "Not evaluated" is not the same as "no category": the first means no
   * simulation has run, the second that no rule matched. Showing an empty cell
   * for either loses the difference.
   */
  it('says an item has not been evaluated rather than leaving the cell blank', async () => {
    show([media({ computed_category: null, override_category: null })]);

    expect(await screen.findByText('not evaluated')).toBeTruthy();
  });

  /**
   * Missing metadata is a warning because it stops genre and keyword rules from
   * matching at all — the item will route on nothing and the user will not know
   * why. Having it is not an achievement and carries no colour.
   */
  it('warns about missing metadata', async () => {
    show([media({ has_metadata: false })]);

    const warning = await screen.findByText('missing');
    expect(warning.className).toContain('badge-warning');
  });

  it('gives cached metadata no colour, because having it is not an outcome', async () => {
    show([media({ has_metadata: true })]);

    const badge = await screen.findByText('cached');
    expect(badge.className).not.toContain('badge-warning');
    expect(badge.className).toContain('muted');
  });

  it('names the explain button after its row', async () => {
    show([media({ title: 'Perfect Blue' })]);

    expect(await screen.findByRole('button', { name: 'Why? Perfect Blue' })).toBeTruthy();
  });

  it('asks the server to explain the row that was clicked', async () => {
    const explain = vi.spyOn(api, 'explainMedia').mockResolvedValue({
      media: { ...media({ id: 'm7' }), current_path: '/data/films/Akira', added_at: null },
      metadata: null,
      override_category: null,
      target_category: 'anime',
      target_root_folder: '/data/anime',
      action: 'move',
      confidence: 0.9,
      winning_rule: 'Japanese',
      rule_traces: [],
    });
    show([media({ id: 'm7', title: 'Akira' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Why? Akira' }));

    await waitFor(() => expect(explain).toHaveBeenCalledWith('m7'));
  });

  /**
   * The list is paginated server-side. A type filter applied in the browser
   * would filter one page of fifty and present it as the whole answer.
   */
  it('asks the server for the filtered set rather than hiding rows on screen', async () => {
    const getMedia = show([media()]);
    await screen.findByText('Akira');

    await userEvent.selectOptions(screen.getByLabelText('Filter by type'), 'series');

    await waitFor(() =>
      expect(getMedia).toHaveBeenCalledWith(expect.objectContaining({ media_type: 'series' })),
    );
  });

  it('returns to the first page when the filter changes', async () => {
    const getMedia = show([media()], 2);
    await screen.findByText('Akira');
    // From page 2, or the assertion below holds on a page that never moved.
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() =>
      expect(getMedia).toHaveBeenLastCalledWith(expect.objectContaining({ page: 2 })),
    );

    await userEvent.selectOptions(screen.getByLabelText('Filter by type'), 'movie');

    // Otherwise a narrower filter lands the user on page 7 of 2, which renders
    // as an empty library.
    await waitFor(() =>
      expect(getMedia).toHaveBeenLastCalledWith(expect.objectContaining({ page: 1 })),
    );
  });
});
