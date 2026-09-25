import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { decision } from '../test/fixtures';
import DecisionRowHarness from '../test/DecisionRowHarness.svelte';

/**
 * One row of a simulation. It is the smallest place where the explainability
 * contract is visible: the reasons the engine gave, the alternatives it
 * discarded, and whether this one is actionable at all.
 */

const STRINGS = {
  SelectMoveFor: 'Select the move for "{title}"',
  Movies: 'Movies',
  Series: 'Series',
  ManualOverride: 'manual override',
  DefaultCategoryFallback: 'default category',
  NoFolderForCategory: 'no folder for "{category}"',
  AlsoMatched: '{rule} also matched, for {category}',
  ExcludedAlternative: '{rule} was vetoed by {reason}',
  None: '-',
};

const show = (props: Record<string, unknown>) =>
  renderWithI18n(DecisionRowHarness, { props, strings: STRINGS });

afterEach(() => vi.restoreAllMocks());

describe('DecisionRow', () => {
  it('offers a checkbox only for a decision that would move something', () => {
    show({ decision: decision({ action: 'move' }), selected: false, onToggle: vi.fn() });

    expect(screen.getByRole('checkbox', { name: /Select the move for/ })).toBeTruthy();
  });

  it('offers none for a decision that is already correct', () => {
    show({ decision: decision({ action: 'none' }), selected: false, onToggle: vi.fn() });

    expect(screen.queryByRole('checkbox')).toBeNull();
  });

  it('offers none for a skip, which the executor could only refuse', () => {
    show({
      decision: decision({ action: 'skip', target_root_folder: null }),
      selected: false,
      onToggle: vi.fn(),
    });

    expect(screen.queryByRole('checkbox')).toBeNull();
  });

  /**
   * A category with no root folder behind it is the single most common reason
   * nothing moves, and the row is where it has to be said — not only in a
   * warning above a table the user scrolled past.
   */
  it('says which category has no folder rather than leaving the cell empty', () => {
    show({
      decision: decision({ target_root_folder: null, target_category: 'anime' }),
      selected: false,
      onToggle: vi.fn(),
    });

    expect(screen.getByText(/no folder for "anime"/)).toBeTruthy();
  });

  it('names the fallback when no rule matched', () => {
    show({
      decision: decision({ matched_rule_name: null }),
      selected: false,
      onToggle: vi.fn(),
    });

    expect(screen.getByText('default category')).toBeTruthy();
  });

  it('marks a decision a human pinned', () => {
    show({ decision: decision({ is_override: true }), selected: false, onToggle: vi.fn() });

    expect(screen.getByText('manual override')).toBeTruthy();
  });

  /**
   * The rules that lost are part of the explanation. Showing only the winner
   * makes a surprising outcome impossible to argue with.
   */
  it('shows the rules that also matched, and the ones that were vetoed', () => {
    show({
      decision: decision({
        alternatives: [
          { rule_name: 'Kids', category: 'kids', excluded_by: null, reason: '', confidence: 0.5 },
          {
            rule_name: 'Concerts',
            category: 'concerts',
            excluded_by: 'certification G',
            reason: '',
            confidence: 0.4,
          },
        ],
      }),
      selected: false,
      onToggle: vi.fn(),
    });

    expect(screen.getByText(/Kids also matched, for kids/)).toBeTruthy();
    expect(screen.getByText(/Concerts was vetoed by certification G/)).toBeTruthy();
  });

  it('keeps the whole justification available on hover, clamped on screen', () => {
    const reason = '✓ Original language in [ja] – found [ja]';
    show({ decision: decision({ reasons: [reason] }), selected: false, onToggle: vi.fn() });

    const line = screen.getByText(reason);
    expect(line.getAttribute('title')).toBe(reason);
    expect(line.className).toContain('reason-line');
  });
});
