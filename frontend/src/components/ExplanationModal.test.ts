import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { dropFocus } from '../test/focus';
import { nthCall } from '../test/spy';
import { category, explainedMedia, explanation } from '../test/fixtures';
import type { Explanation, MetadataProvider } from '../api/types';
import { ApiError, api } from '../api/client';
import ExplanationModal from './ExplanationModal.svelte';
import { publishAccess } from '../lib/access.svelte';

/**
 * The panel, the source catalogue left unanswered unless a test answers it:
 * until it does, each source is named by its id.
 */
function show(
  data: Explanation,
  strings?: Record<string, string>,
  onChanged = vi.fn(),
  direction: 'ltr' | 'rtl' = 'ltr',
) {
  if (!vi.isMockFunction(api.getSources)) {
    vi.spyOn(api, 'getSources').mockReturnValue(new Promise(() => {}));
  }
  if (!vi.isMockFunction(api.getConditionCatalog)) {
    vi.spyOn(api, 'getConditionCatalog').mockResolvedValue({
      match_modes: ['all', 'any'],
      conditions: [],
      score_scales: [{ id: 'imdb', name: 'IMDb', max: 10, votes: true }],
    });
  }
  if (!vi.isMockFunction(api.getCategories)) {
    const names = ['anime', 'kids', 'standard'];
    vi.spyOn(api, 'getCategories').mockResolvedValue(names.map((name) => category({ name })));
  }
  return renderWithI18n(ExplanationModal, {
    props: { data, onClose: vi.fn(), onChanged },
    strings,
    language: direction === 'rtl' ? 'ar' : 'en',
    direction,
  });
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
  it('opens on its heading, not on a button that writes', async () => {
    show(explanation());

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('heading', { level: 2 })),
    );
  });

  it('lists what the Arr reports that a rule can read, and leaves out what it does not', () => {
    const media = explainedMedia({ quality_profile: 'Ultra-HD', runtime: 86, studio: 'Ghibli' });
    show(explanation({ media }));

    const text = (role: string) => screen.getAllByRole(role).map((each) => each.textContent);
    expect(text('term')).toEqual(['Quality profile', 'Runtime', 'Studio']);
    expect(text('definition')).toEqual(['Ultra-HD', '86 min', 'Ghibli']);
  });

  it('says beside each action what it does', () => {
    show(explanation());

    expect(
      screen.getByRole('button', { name: 'Ask the sources again' }),
    ).toHaveAccessibleDescription(
      'Reads this title again from every source, and searches AniList and MyAnimeList for it anew.',
    );
    expect(screen.getByRole('button', { name: 'Pin as test' })).toHaveAccessibleDescription(
      'Require this title to keep landing in this category, and report it if a rule change moves it. While a test fails, automatic application waits.',
    );
    expect(
      screen.getByText(
        'Reads this title again from every source, and searches AniList and MyAnimeList for it anew.',
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Keep here' })).toHaveAccessibleDescription(
      'Pins the title to "standard", the category of the folder it is in, so no run proposes to move it.',
    );
  });

  it("keeps the title in its folder's category, and reloads what is behind", async () => {
    const kept = vi.spyOn(api, 'keepTitle').mockResolvedValue({} as never);
    const read = vi
      .spyOn(api, 'explainTitle')
      .mockResolvedValue(explanation({ override_category: 'standard', action: 'none' }));
    const onChanged = vi.fn();
    show(explanation(), undefined, onChanged);

    await fireEvent.click(screen.getByRole('button', { name: 'Keep here' }));

    expect(
      await screen.findByText('Kept in "standard": an exception holds the title there.'),
    ).toBeInTheDocument();
    expect(kept).toHaveBeenCalledWith('m1');
    expect(read).toHaveBeenCalledWith('m1', expect.anything());
    expect(onChanged).toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Keep here' })).toBeDisabled();
    expect(screen.getByText('Kept in "standard" by an exception.')).toBeInTheDocument();
  });

  it('offers no keep when the folder leads to no category, and says where to map it', () => {
    show(explanation({ current_category: null }));

    expect(screen.getByRole('button', { name: 'Keep here' })).toBeDisabled();
    expect(screen.getByText(/leads to no category/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open categories and folders' })).toHaveAttribute(
      'href',
      '/categories',
    );
  });

  it('asks before replacing an exception that sends the title elsewhere', async () => {
    const kept = vi.spyOn(api, 'keepTitle').mockResolvedValue({} as never);
    vi.spyOn(api, 'explainTitle').mockResolvedValue(explanation());
    show(explanation({ override_category: 'kids', target_category: 'kids' }));

    await fireEvent.click(screen.getByRole('button', { name: 'Keep here' }));
    expect(kept).not.toHaveBeenCalled();
    expect(await answerConfirmation()).toBe(
      'An exception already sends this title to "kids". Replace it with one that keeps the title where it is?',
    );

    await waitFor(() => expect(kept).toHaveBeenCalledWith('m1'));
  });

  /** A test may expect what the rules do not give yet: it fails until a rule does. */
  it('pins a test expecting another category, named and explained as such', async () => {
    const pin = vi.spyOn(api, 'pinRuleTest').mockResolvedValue({} as never);
    show(explanation());
    const select = await screen.findByLabelText('Expected category');
    await waitFor(() => expect(within(select).getAllByRole('option')).toHaveLength(3));

    await fireEvent.change(select, { target: { value: 'kids' } });

    expect(screen.getByRole('button', { name: 'Pin as test' })).toHaveAccessibleDescription(
      'Fails until a rule sends the title to "kids". While it fails, automatic application waits.',
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Pin as test' }));
    expect(pin).toHaveBeenCalledWith('Akira belongs in kids', 'm1', 'kids');
  });

  /** A rule test replays the rules alone, so under an exception it pins what they decide. */
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

  /**
   * The name a pinned test is kept under is compared and shown in either direction later, so it
   * carries none of the isolate marks a right-to-left sentence sets around its values.
   */
  it('pins a test under a name with no isolate mark in Arabic, its hint still isolated', async () => {
    const pin = vi.spyOn(api, 'pinRuleTest').mockResolvedValue({} as never);
    const marks = /[\u2066-\u2069]/;
    const strings = { PinnedCaseName: '{title} في {category}', KeepHereHint: 'يبقى في {category}' };
    show(explanation(), strings, vi.fn(), 'rtl');

    await fireEvent.click(screen.getByRole('button', { name: /pin as test/i }));

    expect(nthCall(pin)[0]).toBe('Akira في anime');
    expect(nthCall(pin)[0]).not.toMatch(marks);
    expect(screen.getByText(/^يبقى في/).textContent).toMatch(marks);
  });

  /** A refusal drawn as a red box with no role would reach no screen reader. */
  it('announces a pin that was refused', async () => {
    vi.spyOn(api, 'pinRuleTest').mockRejectedValue(new ApiError('already a case', 409, 'conflict'));
    show(explanation());

    await fireEvent.click(screen.getByRole('button', { name: /pin as test/i }));

    expect(await screen.findByRole('alert')).toHaveTextContent('already a case');
  });

  it('reads the explanation again once the sources were asked again', async () => {
    const asked = vi.spyOn(api, 'refreshTitleMetadata').mockResolvedValue({ answered: 2 });
    vi.spyOn(api, 'explainTitle').mockResolvedValue(explanation({ target_category: 'kids' }));
    show(explanation());

    await fireEvent.click(screen.getByRole('button', { name: /ask the sources again/i }));

    expect(await screen.findByText('Sources that answered: 2')).toBeTruthy();
    expect(screen.getByText('kids', { selector: '.badge' })).toBeTruthy();
    expect(asked).toHaveBeenCalledWith(explanation().media.id, expect.anything());
  });

  /** The sources may take a while, and the panel closed has nobody to tell. */
  it('stops following the refresh once the panel is closed', async () => {
    const asked = vi.spyOn(api, 'refreshTitleMetadata').mockReturnValue(new Promise(() => {}));
    const { unmount } = show(explanation());
    await fireEvent.click(screen.getByRole('button', { name: /ask the sources again/i }));

    unmount();

    expect(nthCall(asked)[1]?.signal?.aborted).toBe(true);
  });

  /**
   * Each action turns disabled while it runs, Keep here and Pin as test for good once done: the
   * focus goes back to the button, else to the panel's heading, never to the page.
   */
  it.each([
    ['Ask the sources again', 'Ask the sources again'],
    ['Keep here', 'Akira'],
    ['Pin as test', 'Akira'],
  ])('hands the focus on once %s is done', async (action, next) => {
    let release: () => void = () => {};
    const answered = new Promise<void>((resolve) => (release = resolve));
    const later = <T>(value: T) => answered.then(() => value);
    vi.spyOn(api, 'refreshTitleMetadata').mockReturnValue(later({ answered: 1 }));
    vi.spyOn(api, 'keepTitle').mockReturnValue(later({} as never));
    vi.spyOn(api, 'pinRuleTest').mockReturnValue(later({} as never));
    vi.spyOn(api, 'explainTitle').mockResolvedValue(
      explanation({ override_category: 'standard', current_category: 'standard' }),
    );
    show(explanation());
    const pressed = screen.getByRole('button', { name: action });
    pressed.focus();

    await fireEvent.click(pressed);
    dropFocus();
    release();

    await waitFor(() => expect(document.activeElement).toHaveAccessibleName(next));
  });

  it('says so when no source answered a refresh', async () => {
    vi.spyOn(api, 'refreshTitleMetadata').mockResolvedValue({ answered: 0 });
    vi.spyOn(api, 'explainTitle').mockResolvedValue(explanation());
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

  /** Without metadata, genre and keyword rules cannot match, which the panel has to say. */
  it('says no metadata is cached rather than showing an empty panel', () => {
    show(explanation({ metadata: null }));

    expect(
      screen.getByText(
        'No external metadata cached, so genre, keyword and certification conditions cannot match.',
      ),
    ).toBeTruthy();
    expect(screen.queryByText('Metadata')).toBeNull();
  });

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
      // French spacing, so a colon written in the markup could not pass for the sentence.
      { MetadataSourcesLine: 'Sources : {sources}' },
    );

    expect(screen.getByText('Animation')).toBeTruthy();
    expect(screen.getByText(/Sources/)).toHaveTextContent(/^Sources : arr\s*→\s*tmdb$/);
  });

  it('says how each anime source found the title', () => {
    show(
      explanation({
        metadata: {
          genres: [],
          keywords: [],
          origin_countries: [],
          field_sources: {},
          sources: ['anilist', 'tenrai'],
        } as unknown as Explanation['metadata'],
        identified_by: { anilist: 'search', tenrai: 'mapping' },
      }),
    );

    expect(screen.getByText(/^anilist found it by its title and year/)).toBeTruthy();
    expect(
      screen.getByText('tenrai found it by its identifiers, from the anime identifier table.'),
    ).toBeTruthy();
  });

  it('lists each score in the catalogue order, its votes beside it, in the reader number form', async () => {
    show(
      explanation({
        metadata: {
          genres: ['Animation'],
          keywords: [],
          original_language: 'ja',
          origin_countries: [],
          certification: null,
          ratings: { metacritic: { value: 86, votes: null }, imdb: { value: 8, votes: 393000 } },
          field_sources: {},
          sources: ['arr'],
        } as unknown as Explanation['metadata'],
      }),
    );

    // A scale the catalogue does not name keeps its id and comes last, and a score out of 10
    // keeps its tenth.
    const terms = () => screen.getAllByRole('term').map((term) => term.textContent);
    await waitFor(() => expect(terms()).toEqual(['IMDb', 'metacritic']));
    expect(screen.getAllByRole('definition').map((value) => value.textContent)).toEqual([
      '8.0 (393,000)',
      '86',
    ]);
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
      // Not a comma, which a comma written in the markup would produce as well.
      { ListSeparator: ' ; ' },
    );

    expect(screen.getByText(/Language: Japanese \(ja\)/)).toHaveTextContent(
      /Countries: Japan \(JP\) ; South Korea \(KR\)/,
    );
  });

  it('shows the status and the synopsis, each linked to its source', async () => {
    vi.spyOn(api, 'getSources').mockResolvedValue({
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
          ratings: {},
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

  it('shows a failed condition with what was observed, not only that it failed', async () => {
    vi.spyOn(api, 'getSources').mockResolvedValue({
      providers: [source('tmdb', 'TMDB', 'https://www.themoviedb.org')],
      order: ['tmdb'],
    });
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

    // Scoped to the row: "en" is two letters and matches half the prose on the panel.
    const row = screen
      .getByText('Original language in [ja]')
      .closest('.explain-row') as HTMLElement;
    expect(within(row).getByText(/en/)).toBeTruthy();
    // Named as the panel names a source everywhere else, never by the engine's id.
    expect(await within(row).findByText('from TMDB')).toBeTruthy();
    // Said in a word, as the glyph and its colour reach no screen reader.
    expect(within(row).getByText('did not hold')).toHaveClass('visually-hidden');
    expect(within(row).getByText('✗')).toHaveAttribute('aria-hidden', 'true');
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

/** Asking the sources, keeping a title and pinning a case all write, which a reader is refused. */
describe('for a reader', () => {
  const ACTIONS = ['Ask the sources again', 'Keep here', 'Pin as test'];
  const offered = () => ACTIONS.filter((name) => screen.queryByRole('button', { name }));

  it('offers the owner the three actions, and a reader the explanation alone', async () => {
    const { unmount } = show(explanation());
    await screen.findByRole('heading', { name: 'Akira' });
    expect(offered()).toEqual(ACTIONS);
    unmount();

    publishAccess(true);
    show(explanation());

    await screen.findByRole('heading', { name: 'Akira' });
    expect(screen.getByText('/data/anime')).toBeInTheDocument();
    expect(offered()).toEqual([]);
    expect(screen.queryByLabelText('Expected category')).toBeNull();
  });
});
