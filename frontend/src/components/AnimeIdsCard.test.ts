import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { api } from '../api/client';
import type { AnimeIdsState } from '../api/types';
import { publishAccess } from '../lib/access.svelte';
import { renderWithI18n } from '../test/render';
import { dropFocus } from '../test/focus';
import { nthCall } from '../test/spy';
import AnimeIdsCard from './AnimeIdsCard.svelte';

const NONE: AnimeIdsState = {
  loaded_at: null,
  entries: null,
  origin: null,
  address: 'https://example.test/table.json',
};
const HELD: AnimeIdsState = {
  loaded_at: '2026-10-20T10:00:00Z',
  entries: 21000,
  origin: 'https://example.test/table.json',
  address: 'https://example.test/table.json',
};

afterEach(() => {
  vi.restoreAllMocks();
  publishAccess(null);
});

describe('AnimeIdsCard', () => {
  it('says no table is held, and where a download would ask', async () => {
    vi.spyOn(api, 'getAnimeIds').mockResolvedValue(NONE);
    renderWithI18n(AnimeIdsCard);

    expect(
      await screen.findByText('No table loaded: anime are found by their title and year.'),
    ).toBeTruthy();
    expect(screen.getByText(/Download asks https:\/\/example\.test\/table\.json/)).toBeTruthy();
  });

  it('downloads on request, says what the load did, and reads the table again', async () => {
    const read = vi.spyOn(api, 'getAnimeIds').mockResolvedValue(NONE);
    const download = vi
      .spyOn(api, 'downloadAnimeIds')
      .mockResolvedValue({ entries: 21000, titles: 3 });
    renderWithI18n(AnimeIdsCard);

    read.mockResolvedValue(HELD);
    await fireEvent.click(await screen.findByRole('button', { name: 'Download now' }));

    await screen.findByText(
      'Anime identifiers loaded. Entries: 21,000. Titles given their identifiers: 3.',
    );
    expect(download).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.getByText(/^Entries: 21,000, loaded/)).toBeTruthy());
    expect(screen.getByText('From https://example.test/table.json.')).toBeTruthy();
  });

  /** The table is large, and the card gone has nobody to tell. */
  it('stops following the load once the card is gone', async () => {
    vi.spyOn(api, 'getAnimeIds').mockResolvedValue(NONE);
    const download = vi.spyOn(api, 'downloadAnimeIds').mockReturnValue(new Promise(() => {}));
    const { unmount } = renderWithI18n(AnimeIdsCard);
    await fireEvent.click(await screen.findByRole('button', { name: 'Download now' }));

    unmount();

    expect(nthCall(download)[0]?.signal?.aborted).toBe(true);
  });

  /** Download turns disabled while the table loads. */
  it('hands the focus back to Download once the table is loaded', async () => {
    vi.spyOn(api, 'getAnimeIds').mockResolvedValue(NONE);
    let finish: (loaded: { entries: number; titles: number }) => void = () => {};
    vi.spyOn(api, 'downloadAnimeIds').mockReturnValue(new Promise((resolve) => (finish = resolve)));
    renderWithI18n(AnimeIdsCard);
    const pressed = await screen.findByRole('button', { name: 'Download now' });
    pressed.focus();
    await fireEvent.click(pressed);
    dropFocus();

    finish({ entries: 2, titles: 1 });

    await waitFor(() => expect(document.activeElement).toBe(pressed));
  });

  it('imports a file a person saved', async () => {
    vi.spyOn(api, 'getAnimeIds').mockResolvedValue(NONE);
    const imported = vi.spyOn(api, 'importAnimeIds').mockResolvedValue({ entries: 2, titles: 1 });
    renderWithI18n(AnimeIdsCard);
    const file = new File(['[]'], 'anime-list-mini.json', { type: 'application/json' });

    const input = (await screen.findByLabelText('Import a file')) as HTMLInputElement;
    await fireEvent.change(input, { target: { files: [file] } });

    await waitFor(() => expect(imported).toHaveBeenCalledWith(file, expect.anything()));
  });

  it('offers nothing to load to a reader', async () => {
    publishAccess(true);
    vi.spyOn(api, 'getAnimeIds').mockResolvedValue(HELD);
    renderWithI18n(AnimeIdsCard);

    await screen.findByText(/^Entries: 21,000, loaded/);
    expect(screen.queryByRole('button', { name: 'Download now' })).toBeNull();
  });
});
