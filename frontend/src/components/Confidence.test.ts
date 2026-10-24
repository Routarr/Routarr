import { describe, it, expect } from 'vitest';

import { renderWithI18n } from '../test/render';
import Confidence from './Confidence.svelte';

const show = (value: number, meter = false) =>
  renderWithI18n(Confidence, { props: { value, meter } });

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

  it('flags the one value an empty bar cannot show', () => {
    expect(show(0).container.querySelector('.confidence-value')?.className).toContain('is-none');
    expect(show(0.45).container.querySelector('.confidence-value')?.className).not.toContain(
      'is-none',
    );
  });

  it('leaves the mark out of the accessibility tree', () => {
    expect(show(0.7).container.querySelector('.confidence-dot')?.getAttribute('aria-hidden')).toBe(
      'true',
    );
    expect(
      show(0.7, true).container.querySelector('.confidence-track')?.getAttribute('aria-hidden'),
    ).toBe('true');
  });

  it('draws the meter as a length alone', () => {
    const fill = show(0.7, true).container.querySelector('.confidence-fill') as HTMLElement;

    expect(fill.style.width).toBe('70%');
    expect(fill.style.getPropertyValue('--mix')).toBe('');
  });

  it('carries a meter only where there is room to read one', () => {
    const table = show(0.7).container;
    expect(table.querySelector('.confidence-track')).toBeNull();
    expect(table.querySelector('.confidence-dot')).not.toBeNull();

    const panel = show(0.7, true).container;
    expect(panel.querySelector('.confidence-track')).not.toBeNull();
    expect(panel.querySelector('.confidence-dot')).toBeNull();
  });
});
