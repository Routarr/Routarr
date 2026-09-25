import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { media } from '../test/fixtures';
import type { Explanation } from '../api/types';
import { ApiError, api } from '../api/client';
import ExplanationModal from './ExplanationModal.svelte';

/**
 * The explainability contract, made visible. Every condition reports what was
 * expected and what was observed *even when it failed* — that is the whole
 * point, and it is what `/media/{id}/explain` exists to serve.
 */

const STRINGS = {
  PinAsRuleTest: 'Pin as a rule test',
  ProposedCategory: 'Proposed category',
  Confidence: 'Confidence',
  ManualOverride: 'manual override',
  MetadataTitle: 'Metadata',
  MetadataSources: 'Sources',
  MetadataSummary: 'Language: {language} · Countries: {countries} · Certification: {certification}',
  MetadataFromSource: 'from {source}',
  NoMetadataCached: 'No metadata cached',
  RuleEvaluation: 'Rule evaluation',
  NoRuleApplies: 'No rule applies',
  OutcomeWinner: 'winner',
  OutcomeExcluded: 'excluded',
  OutcomeLowerPriority: 'lower priority',
  OutcomeNotMatched: 'not matched',
  VetoedByExclusion: 'vetoed by {reason}',
  NoRootFolderMapped: 'no root folder',
  Unknown: 'unknown',
  Dismiss: 'Close',
  None: '-',
};

function explanation(over: Partial<Explanation> = {}): Explanation {
  return {
    media: { ...media(), current_path: '/data/films/Akira', added_at: null },
    metadata: null,
    override_category: null,
    target_category: 'anime',
    target_root_folder: '/data/anime',
    action: 'move',
    confidence: 0.7,
    winning_rule: 'Japanese animation',
    rule_traces: [],
    ...over,
  };
}

const show = (data: Explanation) =>
  renderWithI18n(ExplanationModal, { props: { data, onClose: vi.fn() }, strings: STRINGS });

afterEach(() => vi.restoreAllMocks());

describe('ExplanationModal', () => {
  /**
   * A refused pin was a red box with no role: sighted users saw it, a screen
   * reader heard nothing and the button simply came back.
   */
  it('announces a pin that was refused', async () => {
    vi.spyOn(api, 'pinRuleTest').mockRejectedValue(new ApiError('already a case', 409, 'conflict'));
    show(explanation());

    await fireEvent.click(screen.getByRole('button', { name: /pin as a rule test/i }));

    expect(await screen.findByRole('alert')).toHaveTextContent('already a case');
  });

  it('says where the item is and where it would go', () => {
    show(explanation());

    expect(screen.getByText('/data/films')).toBeTruthy();
    expect(screen.getByText('/data/anime')).toBeTruthy();
  });

  it('says so when the target category has no folder behind it', () => {
    show(explanation({ target_root_folder: null }));

    expect(screen.getByText('no root folder')).toBeTruthy();
  });

  it('marks a category a human pinned', () => {
    show(explanation({ override_category: 'kids' }));

    expect(screen.getByText('manual override')).toBeTruthy();
  });

  /**
   * Nothing cached is not the same as nothing to say: genre and keyword rules
   * cannot match at all without metadata, and the user has to be told why.
   */
  it('says no metadata is cached rather than showing an empty panel', () => {
    show(explanation({ metadata: null }));

    expect(screen.getByText('No metadata cached')).toBeTruthy();
    expect(screen.queryByText('Metadata')).toBeNull();
  });

  /**
   * Which sources contributed, in priority order. With one source this is
   * obvious; with several it is the only way to know whether a genre came from
   * the library or from TMDb.
   */
  it('names the sources that contributed, in order', () => {
    show(
      explanation({
        metadata: {
          genres: ['Animation'],
          keywords: ['cyberpunk'],
          original_language: 'ja',
          origin_countries: ['JP'],
          certification: 'R',
          sources: ['arr', 'tmdb'],
        } as Explanation['metadata'],
      }),
    );

    expect(screen.getByText('Animation')).toBeTruthy();
    expect(screen.getByText(/Language: ja/)).toBeTruthy();
    const sources = screen.getByText(/Sources/);
    expect(sources.textContent).toContain('arr');
    expect(sources.textContent).toContain('tmdb');
  });

  it('says no rule applies rather than showing an empty list', () => {
    show(explanation({ rule_traces: [] }));

    expect(screen.getByText('No rule applies')).toBeTruthy();
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
    expect(screen.getByText('not matched')).toBeTruthy();
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

    expect(screen.getByText(/vetoed by certification in \[G\]/)).toBeTruthy();
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
