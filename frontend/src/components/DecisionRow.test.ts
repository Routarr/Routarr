import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { decision } from '../test/fixtures';
import DecisionRowHarness from '../test/DecisionRowHarness.svelte';

const show = (props: Record<string, unknown>) => renderWithI18n(DecisionRowHarness, { props });

afterEach(() => vi.restoreAllMocks());

describe('DecisionRow', () => {
  /** Named after its film, so a screen reader tells one row's box from the next. */
  it('offers a checkbox only for a decision that would move something', () => {
    show({
      decision: decision({ action: 'move', media_title: 'Perfect Blue' }),
      selected: false,
      onToggle: vi.fn(),
    });

    expect(
      screen.getByRole('checkbox', { name: 'Select the move for "Perfect Blue"' }),
    ).toBeTruthy();
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

  /** A category with no folder is said on its row, not only in a warning above the table. */
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

    expect(screen.getByText('Manual exception')).toBeTruthy();
  });

  /** A title in no root folder yet: a dash on screen, a word to a screen reader. */
  it('says an empty folder cell in a word', () => {
    show({
      decision: decision({ current_root_folder: null }),
      selected: false,
      onToggle: vi.fn(),
    });

    expect(screen.getByText('none')).toHaveClass('visually-hidden');
  });

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

    expect(screen.getByText('also matched: Kids → kids')).toBeTruthy();
    expect(screen.getByText(/Concerts excluded \(certification G\)/)).toBeTruthy();
  });

  /** Clamped to two lines, a justification is offered in full behind a button of its row. */
  describe('a justification longer than two lines', () => {
    const reason = '✓ Original language in [ja] – found [ja]';

    /** jsdom lays nothing out: the row's lines are as tall as `scrollHeight` says. */
    function layOut(scrollHeight: number) {
      vi.spyOn(Element.prototype, 'scrollHeight', 'get').mockReturnValue(scrollHeight);
      vi.spyOn(Element.prototype, 'clientHeight', 'get').mockReturnValue(32);
      vi.stubGlobal(
        'ResizeObserver',
        class {
          report: () => void;
          constructor(report: () => void) {
            this.report = report;
          }
          observe() {
            this.report();
          }
          disconnect() {}
        },
      );
    }

    afterEach(() => vi.unstubAllGlobals());

    it('is clamped on screen, and offered in full behind a button', async () => {
      layOut(80);
      show({
        decision: decision({ reason_lines: [{ text: reason, matched: true }] }),
        selected: false,
        onToggle: vi.fn(),
      });

      const line = screen.getByText(reason);
      expect(line.className).toContain('reason-line');
      const toggle = await screen.findByRole('button', { name: 'Show in full – Akira' });
      expect(toggle).toHaveAttribute('aria-expanded', 'false');

      await fireEvent.click(toggle);

      expect(screen.getByRole('button', { name: 'Show less – Akira' })).toHaveAttribute(
        'aria-expanded',
        'true',
      );
      expect(line.closest('.is-expanded')).not.toBeNull();
    });

    it('offers no button when every line fits', async () => {
      layOut(32);
      show({
        decision: decision({ reason_lines: [{ text: reason, matched: true }] }),
        selected: false,
        onToggle: vi.fn(),
      });
      // The lines are measured in the frame after the observer reports.
      await new Promise((frame) => requestAnimationFrame(frame));

      expect(screen.queryByRole('button', { name: /^Show in full/ })).toBeNull();
    });
  });
});

/**
 * A line the title meets is marked as one by the flag the server sends with
 * it, whatever its first character: a translation may move the mark, or open
 * the line with a direction mark.
 */
it('marks the lines the title meets, by what the server says of each', () => {
  show({
    decision: decision({
      reason_lines: [
        { text: '\u200fGenre contains Animation ✓', matched: true },
        { text: '✓ looks met, is not', matched: false },
        { text: 'A sentence a row stored', matched: null },
      ],
    }),
    selected: false,
    onToggle: vi.fn(),
  });

  expect(screen.getByText('\u200fGenre contains Animation ✓').className).toContain('explain-match');
  expect(screen.getByText('✓ looks met, is not').className).toContain('text-muted');
  expect(screen.getByText('A sentence a row stored').className).toContain('text-muted');
});

describe('the action cell', () => {
  it('says the action in words beside its icon', () => {
    for (const [action, word] of [
      ['move', 'Move'],
      ['skip', 'Skip'],
      ['none', 'Already correct'],
    ] as const) {
      const { unmount } = show({
        decision: decision({ action }),
        selected: false,
        onToggle: vi.fn(),
      });
      expect(screen.getByText(word)).toBeTruthy();
      unmount();
    }
  });
});
