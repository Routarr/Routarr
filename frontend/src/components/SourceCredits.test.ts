import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { ApiError, api } from '../api/client';
import type { MetadataProvider } from '../api/types';
import SourceCredits from './SourceCredits.svelte';

/**
 * What each source's terms ask of whoever shows its data: TMDB its notice and
 * logo, TheTVDB a sentence and a link, OMDb its licence. A credit missing is
 * a source used outside its terms.
 */

const STRINGS = {
  SourceCreditsTitle: 'Data sources',
  CreditTmdb: 'Routarr uses TMDB and the TMDB APIs but is not endorsed by TMDB.',
  CreditTvdb: 'Metadata provided by TheTVDB.',
  CreditOmdb: 'Data from OMDb, licensed under CC BY-NC 4.0.',
  CreditAnilist: 'Data from AniList.',
  CreditJikan: 'Data from MyAnimeList, read through Jikan.',
  TmdbLogo: 'TMDB logo',
};

function source(id: string, website: string | null): MetadataProvider {
  return {
    id,
    display_name: id,
    fetched: website !== null,
    needs_key: false,
    key_env: null,
    configured: false,
    fields: [],
    media_types: ['movie', 'series'],
    website,
  };
}

const CATALOGUE = {
  providers: [
    source('arr', null),
    source('tmdb', 'https://www.themoviedb.org'),
    source('anilist', 'https://anilist.co'),
    source('jikan', 'https://myanimelist.net'),
    source('omdb', 'https://www.omdbapi.com'),
    source('tvdb', 'https://thetvdb.com'),
  ],
  order: ['arr'],
};

afterEach(() => vi.restoreAllMocks());

describe('SourceCredits', () => {
  it('credits every source as its terms ask, enabled or not, and links to it', async () => {
    vi.spyOn(api, 'getMetadataProviders').mockResolvedValue(CATALOGUE);
    renderWithI18n(SourceCredits, { strings: STRINGS });

    const tmdb = (await screen.findByText(STRINGS.CreditTmdb)).closest('li')!;
    expect(within(tmdb).getByRole('img', { name: 'TMDB logo' })).toBeTruthy();
    expect(within(tmdb).getByRole('link')).toHaveAttribute('href', 'https://www.themoviedb.org');

    const tvdb = screen.getByText(STRINGS.CreditTvdb).closest('li')!;
    expect(within(tvdb).getByRole('link', { name: 'thetvdb.com' })).toHaveAttribute(
      'href',
      'https://thetvdb.com',
    );
    // The licence's name in the sentence is its link, said once.
    const licence = screen.getByRole('link', { name: 'CC BY-NC 4.0' });
    expect(licence).toHaveAttribute('href', 'https://creativecommons.org/licenses/by-nc/4.0/');
    const omdb = licence.closest('li')!;
    expect(omdb.textContent?.split('CC BY-NC 4.0')).toHaveLength(2);
    expect(within(omdb).getByRole('link', { name: 'www.omdbapi.com' })).toBeTruthy();
    expect(screen.getByText(STRINGS.CreditAnilist)).toBeTruthy();
    expect(screen.getByText(STRINGS.CreditJikan)).toBeTruthy();
    // The Arrs supply their own library: nothing of theirs to credit.
    expect(screen.getAllByRole('listitem')).toHaveLength(5);
  });

  it('says a catalogue that could not be read', async () => {
    vi.spyOn(api, 'getMetadataProviders').mockRejectedValue(
      new ApiError('The server is unreachable.', 0, 'network'),
    );
    renderWithI18n(SourceCredits, { strings: STRINGS });

    expect(await screen.findByRole('alert')).toHaveTextContent('The server is unreachable.');
  });
});
