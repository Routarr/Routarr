import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { explainedMedia } from '../test/fixtures';
import type { Explanation, MetadataProvider } from '../api/types';
import { ApiError, api } from '../api/client';
import ExplanationModal from './ExplanationModal.svelte';

/**
 * The explainability contract, made visible. Every condition reports what was
 * expected and what was observed *even when it failed*. That is the whole
 * point, and it is what `/media/{id}/explain` exists to serve.
 */

function explanation(over: Partial<Explanation> = {}): Explanation {
  return {
    media: explainedMedia({ current_path: '/data/films/Akira' }),
    metadata: null,
    override_category: null,
    target_category: 'anime',
    rules_category: 'anime',
    target_root_folder: '/data/anime',
    action: 'move',
    confidence: 0.7,
    winning_rule: 'Japanese animation',
    rule_traces: [],
    instance_enabled: true,
    ...over,
  };
}

/**
 * The panel, the source catalogue left unanswered unless a test answers it:
 * until it does, each source is named by its id.
 */
function show(data: Explanation, strings?: Record<string, string>) {
  if (!vi.isMockFunction(api.getMetadataProviders)) {
    vi.spyOn(api, 'getMetadataProviders').mockReturnValue(new Promise(() => {}));
  }
  return renderWithI18n(ExplanationModal, { props: { data, onClose: vi.fn() }, strings });
}

function source(id: string, display_name: string, website: string | null): MetadataProvider {
  return {
    id,
    display_name,
    fetched: website !== null,
    needs_key: false,
    key_env: null,
    configured: true,
    fields: [],
    media_types: ['movie', 'series'],
    website,
  };
}

afterEach(() => vi.restoreAllMocks());

describe('ExplanationModal', () => {
  /**
   * Opened from the quick search by Enter, a panel that focused its first
   * button would take a second Enter as a pin nobody asked for.
   */
  it('opens on its heading, not on a button that writes', async () => {
    show(explanation());

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('heading', { level: 2 })),
    );
  });

  /** What each action does is said beside it, not in a title only a mouse shows. */
  it('says beside each action what it does', () => {
    show(explanation());

    expect(
      screen.getByRole('button', { name: 'Ask the sources again' }),
    ).toHaveAccessibleDescription(
      'Reads this title again from every source, and searches AniList and MyAnimeList for it anew.',
    );
    expect(screen.getByRole('button', { name: 'Pin as test' })).toHaveAccessibleDescription(
      'Require this title to keep landing in this category, and report it if a rule change moves it.',
    );
    expect(
      screen.getByText(
        'Reads this title again from every source, and searches AniList and MyAnimeList for it anew.',
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        'Require this title to keep landing in this category, and report it if a rule change moves it.',
      ),
    ).toBeInTheDocument();
  });

  /**
   * A refused pin has to be announced. Drawn as a red box with no role, it is
   * seen by sighted users, heard by no screen reader, and the button simply
   * comes back.
   */
  /** A rule test replays the rules alone: under an exception it pins what they decide, and says so. */
  it('pins what the rules decide when an exception forces the title elsewhere', async () => {
    const pin = vi.spyOn(api, 'pinRuleTest').mockResolvedValue({} as never);
    show(
      explanation({ override_category: 'kids', target_category: 'kids', rules_category: 'anime' }),
    );

    expect(
      screen.getByText(
        'An exception forces this title, so the test pins what the rules decide: anime.',
      ),
    ).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: /pin as test/i }));
    expect(pin).toHaveBeenCalledWith(expect.any(String), expect.any(String), 'anime');
  });

  it('pins the category on display, and says nothing more, when no exception is in force', async () => {
    const pin = vi.spyOn(api, 'pinRuleTest').mockResolvedValue({} as never);
    show(explanation());

    expect(screen.queryByText(/An exception forces this title/)).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: /pin as test/i }));
    expect(pin).toHaveBeenCalledWith(expect.any(String), expect.any(String), 'anime');
  });

  it('announces a pin that was refused', async () => {
    vi.spyOn(api, 'pinRuleTest').mockRejectedValue(new ApiError('already a case', 409, 'conflict'));
    show(explanation());

    await fireEvent.click(screen.getByRole('button', { name: /pin as test/i }));

    expect(await screen.findByRole('alert')).toHaveTextContent('already a case');
  });

  /**
   * A refresh shows what the sources say now: the explanation is read again
   * once they have answered, and how many did is said, a refresh none of them
   * answered included.
   */
  it('reads the explanation again once the sources were asked again', async () => {
    const asked = vi.spyOn(api, 'refreshTitleMetadata').mockResolvedValue({ answered: 2 });
    vi.spyOn(api, 'explainMedia').mockResolvedValue(explanation({ target_category: 'kids' }));
    show(explanation());

    await fireEvent.click(screen.getByRole('button', { name: /ask the sources again/i }));

    expect(await screen.findByText('Sources that answered: 2')).toBeTruthy();
    expect(screen.getByText('kids')).toBeTruthy();
    expect(asked).toHaveBeenCalledWith(explanation().media.id);
  });

  it('says so when no source answered a refresh', async () => {
    vi.spyOn(api, 'refreshTitleMetadata').mockResolvedValue({ answered: 0 });
    vi.spyOn(api, 'explainMedia').mockResolvedValue(explanation());
    show(explanation());

    await fireEvent.click(screen.getByRole('button', { name: /ask the sources again/i }));

    expect(
      await screen.findByText('No source answered. The next pass asks them again.'),
    ).toBeTruthy();
  });

  it('announces a refresh that was refused', async () => {
    vi.spyOn(api, 'refreshTitleMetadata').mockRejectedValue(
      new ApiError('a pass is running', 409, 'conflict'),
    );
    show(explanation());

    await fireEvent.click(screen.getByRole('button', { name: /ask the sources again/i }));

    expect(await screen.findByRole('alert')).toHaveTextContent('a pass is running');
  });

  it('says where the item is and where it would go', () => {
    show(explanation());

    expect(screen.getByText('/data/films')).toBeTruthy();
    expect(screen.getByText('/data/anime')).toBeTruthy();
  });

  it('says so when the target category has no folder behind it', () => {
    show(explanation({ target_root_folder: null }));

    expect(screen.getByText('no root folder mapped')).toBeTruthy();
  });

  it('says a switched-off instance moves nothing, and only then', () => {
    show(explanation());
    expect(
      screen.queryByText(
        'This instance is switched off: no run reads its titles, so nothing moves.',
      ),
    ).toBeNull();

    show(explanation({ instance_enabled: false }));
    expect(
      screen.getByText('This instance is switched off: no run reads its titles, so nothing moves.'),
    ).toBeTruthy();
  });

  it('marks a category a human pinned', () => {
    show(explanation({ override_category: 'kids' }));

    expect(screen.getByText('Manual exception')).toBeTruthy();
  });

  /**
   * Nothing cached is not the same as nothing to say: genre and keyword rules
   * cannot match at all without metadata, and the user has to be told why.
   */
  it('says no metadata is cached rather than showing an empty panel', () => {
    show(explanation({ metadata: null }));

    expect(
      screen.getByText(
        'No external metadata cached, so genre, keyword and certification conditions cannot match.',
      ),
    ).toBeTruthy();
    expect(screen.queryByText('Metadata')).toBeNull();
  });

  /**
   * Which sources contributed, in priority order. With one source this is
   * obvious. With several it is the only way to know whether a genre came from
   * the library or from TMDB.
   */
  it('names the sources that contributed, in order', () => {
    show(
      explanation({
        metadata: {
          genres: ['Animation'],
          keywords: ['cyberpunk'],
          original_language: 'ja',
          origin_countries: ['JP', 'KR'],
          certification: 'R',
          field_sources: {},
          sources: ['arr', 'tmdb'],
        } as Explanation['metadata'],
      }),
      // French spacing, which the English shares with a colon written in the
      // markup: only a sentence of its own tells the two apart.
      { MetadataSourcesLine: 'Sources : {sources}' },
    );

    expect(screen.getByText('Animation')).toBeTruthy();
    expect(screen.getByText(/Sources/)).toHaveTextContent(/^Sources : arr\s*→\s*tmdb$/);
  });

  /** Named as the rule editor and the rules table name them, the code beside the name. */
  it('names the language and the countries, not only their codes', () => {
    show(
      explanation({
        metadata: {
          genres: [],
          keywords: [],
          original_language: 'ja',
          origin_countries: ['JP', 'KR'],
          certification: null,
          field_sources: {},
          sources: ['arr'],
        } as unknown as Explanation['metadata'],
      }),
      // The English separator is a comma, which a comma written in the markup
      // would produce as well.
      { ListSeparator: ' ; ' },
    );

    expect(screen.getByText(/Language: Japanese \(ja\)/)).toHaveTextContent(
      /Countries: Japan \(JP\) ; South Korea \(KR\)/,
    );
  });

  /**
   * The status reads in the Arrs' words and the synopsis as written, each
   * with the source that gave it, named and linked as its terms ask.
   */
  it('shows the status and the synopsis, each linked to its source', async () => {
    vi.spyOn(api, 'getMetadataProviders').mockResolvedValue({
      providers: [
        source('arr', 'Radarr / Sonarr', null),
        source('tmdb', 'TMDB', 'https://www.themoviedb.org'),
        source('anilist', 'AniList', 'https://anilist.co'),
      ],
      order: ['arr', 'tmdb', 'anilist'],
    });
    show(
      explanation({
        metadata: {
          genres: ['Animation'],
          keywords: [],
          original_language: 'ja',
          origin_countries: [],
          certification: null,
          certification_scale: null,
          status: 'released',
          overview: 'Two sisters move to the country.',
          field_sources: { genres: 'arr', status: 'tmdb', overview: 'anilist' },
          sources: ['arr', 'tmdb', 'anilist'],
        },
      }),
    );

    const status = await screen.findByText(/^Status: Released, from/);
    expect(within(status).getByRole('link', { name: 'TMDB' })).toHaveAttribute(
      'href',
      'https://www.themoviedb.org',
    );
    expect(screen.getByText('Two sisters move to the country.')).toBeTruthy();
    const synopsis = screen.getByText(/^Synopsis from/);
    expect(within(synopsis).getByRole('link', { name: 'AniList' })).toHaveAttribute(
      'href',
      'https://anilist.co',
    );
    const sources = screen.getByText(/^Sources/);
    expect(sources).toHaveTextContent(/Radarr \/ Sonarr\s*→\s*TMDB\s*→\s*AniList/);
    expect(within(sources).queryByRole('link', { name: 'Radarr / Sonarr' })).toBeNull();
  });

  it('says no rule applies rather than showing an empty list', () => {
    show(explanation({ rule_traces: [] }));

    expect(screen.getByText('No rule applies to this media type and instance.')).toBeTruthy();
  });

  /**
   * A condition reports what was expected *and* what was observed even when it
   * failed. Reporting only the matches makes a surprising outcome impossible to
   * argue with.
   */
  it('shows a failed condition with what was observed, not only that it failed', () => {
    show(
      explanation({
        rule_traces: [
          {
            rule_id: 'r1',
            rule_name: 'Japanese animation',
            priority: 10,
            category: 'anime',
            matched: false,
            outcome: 'not_matched',
            excluded_by: null,
            conditions: [
              {
                kind: 'original_language_in',
                key: 'ConditionOriginalLanguage',
                params: { values: 'ja' },
                expected: 'Original language in [ja]',
                observed: 'en',
                matched: false,
                source: 'tmdb',
              },
            ],
          },
        ],
      }),
    );

    // Scoped to the condition's own row: "en" is two letters and matches half
    // the prose on the panel.
    const row = screen
      .getByText('Original language in [ja]')
      .closest('.explain-row') as HTMLElement;
    expect(within(row).getByText(/en/)).toBeTruthy();
    expect(within(row).getByText(/from tmdb/)).toBeTruthy();
    expect(screen.getByText('did not match')).toBeTruthy();
  });

  it('reports the exclusion that vetoed a rule which otherwise matched', () => {
    show(
      explanation({
        rule_traces: [
          {
            rule_id: 'r1',
            rule_name: 'Japanese animation',
            priority: 10,
            category: 'anime',
            matched: true,
            outcome: 'excluded',
            excluded_by: 'certification in [G]',
            conditions: [],
          },
        ],
      }),
    );

    expect(screen.getByText(/vetoed by exclusion: certification in \[G\]/)).toBeTruthy();
    expect(screen.getByText('excluded')).toBeTruthy();
  });

  it('says when a condition observed nothing at all', () => {
    show(
      explanation({
        rule_traces: [
          {
            rule_id: 'r1',
            rule_name: 'Japanese animation',
            priority: 10,
            category: 'anime',
            matched: true,
            outcome: 'winner',
            excluded_by: null,
            conditions: [
              {
                kind: 'genre_contains',
                key: 'ConditionGenreContains',
                params: { values: 'Animation' },
                expected: 'Genre contains [Animation]',
                observed: '',
                matched: false,
                source: null,
              },
            ],
          },
        ],
      }),
    );

    const row = screen
      .getByText('Genre contains [Animation]')
      .closest('.explain-row') as HTMLElement;
    expect(within(row).getByText('unknown')).toBeTruthy();
  });
});
