import { describe, it, expect } from 'vitest';

import { renderWithI18n } from '../test/render';
import Confidence from './Confidence.svelte';

/**
 * The figure is beside the meter, never on it.
 *
 * Centred over the bar, the value sat on the fill on one side of itself and on
 * the track on the other; its contrast measured 16:1 against the track and
 * 1.95:1 against the amber fill, and a text shadow was hiding the difference.
 * Outside, the background is the card, whose contrast the palette test already
 * measures — so these assertions are about *where* the number is, which is what
 * makes that measurement true.
 */
const show = (value: number, meter = false) =>
  renderWithI18n(Confidence, { props: { value, meter }, strings: {} });

describe('Confidence', () => {
  it('puts the figure outside the bar, not over the fill', () => {
    const { container } = show(0.7, true);

    const value = container.querySelector('.confidence-value');
    const track = container.querySelector('.confidence-track');
    expect(value?.textContent?.replace(/\u202f|\u00a0/g, ' ')).toContain('70');
    // Siblings: neither contains the other, so no value ever sits on a fill.
    expect(track?.contains(value as Node)).toBe(false);
    expect(value?.contains(track as Node)).toBe(false);
  });

  /**
   * A ramp, not three bands. `confidence_for` scales differently per match
   * mode — an `all` rule with one condition is 60%, an `any` rule with two is
   * 53% — so no percentage threshold can order them, and the bands it replaced
   * painted an ordinary two-condition rule as a warning.
   */
  it('places each value on a continuous ramp rather than in a band', () => {
    const mix = (value: number) =>
      Number(
        (show(value).container.querySelector('.confidence-dot') as HTMLElement).style
          .getPropertyValue('--mix')
          .replace('%', ''),
      );

    // Strictly increasing across the range the engine actually produces: no
    // two neighbouring values share a colour the way a band would give them.
    const values = [0.45, 0.53, 0.6, 0.7, 0.8, 0.9, 0.98];
    const mixes = values.map(mix);
    for (let i = 1; i < mixes.length; i += 1) {
      expect(mixes[i], `${values[i]} must sit further along than ${values[i - 1]}`).toBeGreaterThan(
        mixes[i - 1] as number,
      );
    }
    expect(mixes[0]).toBe(0);
    expect(mix(1)).toBe(100);
    // Below the floor the engine can produce, the ramp stops rather than
    // running off its own scale.
    expect(mix(0.1)).toBe(0);
  });

  /// Nothing agreed. The bar is empty, so it cannot say it and the figure does.
  it('flags the one value an empty bar cannot show', () => {
    expect(show(0).container.querySelector('.confidence-value')?.className).toContain('is-none');
    expect(show(0.45).container.querySelector('.confidence-value')?.className).not.toContain(
      'is-none',
    );
  });

  /// The mark repeats the figure in a form a screen reader has no use for.
  it('leaves the mark out of the accessibility tree', () => {
    expect(show(0.7).container.querySelector('.confidence-dot')?.getAttribute('aria-hidden')).toBe(
      'true',
    );
    expect(
      show(0.7, true).container.querySelector('.confidence-track')?.getAttribute('aria-hidden'),
    ).toBe('true');
  });

  /**
   * The meter is for the explanation panel, where it has 150px to be read in.
   * A table column gives it 28px after the figure — a stub that measures
   * nothing — so there the same ramp is carried by a dot instead.
   */
  it('carries a meter only where there is room to read one', () => {
    const table = show(0.7).container;
    expect(table.querySelector('.confidence-track')).toBeNull();
    expect(table.querySelector('.confidence-dot')).not.toBeNull();

    const panel = show(0.7, true).container;
    expect(panel.querySelector('.confidence-track')).not.toBeNull();
    expect(panel.querySelector('.confidence-dot')).toBeNull();
  });
});
