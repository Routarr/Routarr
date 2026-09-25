import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import type { ConditionSpec } from '../api/types';
import ConditionValue from './ConditionValue.svelte';

/**
 * One editor per value type, and every one of them named.
 *
 * The caption that identifies a condition is the text to its left, which is not
 * a `<label>` and cannot be: the list renders twice — conditions and exclusions
 * — so a fixed `for` would duplicate an id. The name travels on the control
 * instead, and without it a screen reader announces a bare textbox.
 */

const STRINGS = {
  Yes: 'Yes',
  No: 'No',
  PlaceholderNumberList: 'e.g. 2019, 2020',
  PlaceholderStringList: 'comma separated',
  PlaceholderPath: '/movies/anime',
  PlaceholderYearFrom: 'from',
  PlaceholderYearTo: 'to',
};

const spec = (over: Partial<ConditionSpec> = {}): ConditionSpec => ({
  type: 'genre_contains',
  label: 'Genre contains',
  value_type: 'string_list',
  needs_metadata: true,
  suggestions: '',
  quantifier: '',
  counterpart: '',
  media_types: ['movie', 'series'],
  metadata_field: 'genres',
  available: true,
  ...over,
});

const show = (props: Record<string, unknown>) =>
  renderWithI18n(ConditionValue, { props, strings: STRINGS });

afterEach(() => vi.restoreAllMocks());

describe('ConditionValue', () => {
  it('renders nothing for a condition the catalogue does not describe', () => {
    const { container } = show({ spec: undefined, value: null, onChange: vi.fn() });

    expect(container.querySelector('input, select')).toBeNull();
  });

  it('offers yes and no for a boolean, named after its condition', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ value_type: 'boolean', label: 'Has files' }), value: true, onChange });

    const control = screen.getByLabelText('Has files');
    await userEvent.selectOptions(control, 'false');

    expect(onChange).toHaveBeenCalledWith(false);
  });

  it('sends a number as a number, not as the string the input holds', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ value_type: 'number', label: 'Minimum year' }), value: 2000, onChange });

    await fireEvent.input(screen.getByLabelText('Minimum year'), { target: { value: '2019' } });

    expect(onChange).toHaveBeenCalledWith(2019);
  });

  it('parses a comma-separated list of numbers', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ value_type: 'number_list', label: 'Years' }), value: [], onChange });

    await fireEvent.input(screen.getByLabelText('Years'), { target: { value: '2019, 2020' } });

    expect(onChange).toHaveBeenCalledWith([2019, 2020]);
  });

  it('parses a comma-separated list of strings', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ label: 'Genre contains' }), value: [], onChange });

    await fireEvent.input(screen.getByLabelText('Genre contains'), {
      target: { value: 'Animation, Drama' },
    });

    expect(onChange).toHaveBeenCalledWith(['Animation', 'Drama']);
  });

  it('takes a path as plain text', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ value_type: 'string', label: 'Path contains' }), value: '', onChange });

    await fireEvent.input(screen.getByLabelText('Path contains'), { target: { value: '/anime' } });

    expect(onChange).toHaveBeenCalledWith('/anime');
  });

  /**
   * Two controls for one condition, so the caption alone would name them
   * identically — and a screen reader would offer two spin buttons called
   * "Year between" with no way to tell which end is which.
   */
  it('tells the two ends of a year range apart', async () => {
    const onChange = vi.fn();
    show({
      spec: spec({ value_type: 'year_range', label: 'Year between' }),
      value: { min: null, max: null },
      onChange,
    });

    const from = screen.getByLabelText('Year between – from');
    const to = screen.getByLabelText('Year between – to');
    expect(from).not.toBe(to);

    await fireEvent.input(from, { target: { value: '1990' } });
    expect(onChange).toHaveBeenLastCalledWith({ min: 1990, max: null });

    await fireEvent.input(to, { target: { value: '1999' } });
    expect(onChange).toHaveBeenLastCalledWith({ min: null, max: 1999 });
  });

  it('reads an empty year bound as no bound rather than as zero', async () => {
    const onChange = vi.fn();
    show({
      spec: spec({ value_type: 'year_range', label: 'Year between' }),
      value: { min: 1990, max: null },
      onChange,
    });

    await fireEvent.input(screen.getByLabelText('Year between – from'), { target: { value: '' } });

    expect(onChange).toHaveBeenLastCalledWith({ min: null, max: null });
  });

  /// The engine refuses anything outside these; the field says so before the
  /// server has to, and the browser's own stepper stops at them.
  it('bounds both year fields to what the engine accepts', () => {
    show({ spec: spec({ value_type: 'year_range', label: 'Year between' }), value: {} });

    const ceiling = String(new Date().getFullYear() + 5);
    for (const bound of ['from', 'to']) {
      const field = screen.getByLabelText(`Year between – ${bound}`);
      expect(field).toHaveAttribute('min', '1888');
      expect(field).toHaveAttribute('max', ceiling);
    }
  });
});
