import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import type { ConditionSpec } from '../api/types';
import ConditionValue from './ConditionValue.svelte';
import ConditionValueHarness from '../test/ConditionValueHarness.svelte';
import { backendConstant } from '../test/backend';

const spec = (over: Partial<ConditionSpec> = {}): ConditionSpec => ({
  type: 'genre_contains',
  label: 'Genre contains',
  group: 'metadata',

  label_key: 'ConditionLabelGenreContains',
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

const show = (props: Record<string, unknown>) => renderWithI18n(ConditionValue, { props });

/** The rule engine's bounds, which the fields repeat so the browser offers nothing it refuses. */
const VALIDATION = 'backend/src/services/rule_engine/validation.rs';
const bound = (name: string) => String(backendConstant(VALIDATION, name));

afterEach(() => vi.restoreAllMocks());

/**
 * One editor per value type, every one named. The caption beside a condition cannot be a
 * `<label>`: the list renders twice, so a fixed `for` would repeat an id. The name travels on the
 * control, or a screen reader announces a bare textbox.
 */
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

    expect(onChange).toHaveBeenCalledWith(false, false);
  });

  it('sends a number as a number, not as the string the input holds', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ value_type: 'number', label: 'Minimum year' }), value: 2000, onChange });

    await fireEvent.input(screen.getByLabelText('Minimum year'), { target: { value: '2019' } });

    expect(onChange).toHaveBeenCalledWith(2019, false);
  });

  it('parses a comma-separated list of strings', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ label: 'Genre contains' }), value: [], onChange });

    await fireEvent.input(screen.getByLabelText('Genre contains'), {
      target: { value: 'Animation, Drama' },
    });

    expect(onChange).toHaveBeenCalledWith(['Animation', 'Drama'], false);
  });

  it.each([
    ['number_list', 'Years', '2019,2020', '[2019,2020]'],
    ['string_list', 'Genre contains', 'anime,kids', '["anime","kids"]'],
  ] as const)(
    'a %s shows what is typed, as it is typed',
    async (valueType, label, typed, bound) => {
      renderWithI18n(ConditionValueHarness, {
        props: { spec: spec({ value_type: valueType, label }), initial: [] },
      });
      const field = screen.getByLabelText(label) as HTMLInputElement;

      await userEvent.type(field, typed);

      expect(field.value).toBe(typed);
      expect(screen.getByTestId('bound').textContent).toBe(bound);
    },
  );

  it('a corrected id list keeps its separator', async () => {
    renderWithI18n(ConditionValueHarness, {
      props: { spec: spec({ value_type: 'number_list', label: 'Years' }), initial: [2019, 2020] },
    });
    const field = screen.getByLabelText('Years') as HTMLInputElement;

    await userEvent.type(field, '2021,', { initialSelectionStart: 5, initialSelectionEnd: 5 });

    expect(field.value).toBe('2019,2021, 2020');
    expect(screen.getByTestId('bound').textContent).toBe('[2019,2021,2020]');
  });

  it('takes a path as plain text', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ value_type: 'string', label: 'Path contains' }), value: '', onChange });

    await fireEvent.input(screen.getByLabelText('Path contains'), { target: { value: '/anime' } });

    expect(onChange).toHaveBeenCalledWith('/anime', false);
  });

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
    expect(onChange).toHaveBeenLastCalledWith({ min: 1990, max: null }, false);

    await fireEvent.input(to, { target: { value: '1999' } });
    expect(onChange).toHaveBeenLastCalledWith({ min: null, max: 1999 }, false);
  });

  it('edits a score on its scale, and asks for votes only where the scale counts them', async () => {
    const onChange = vi.fn();
    const scales = [
      { id: 'imdb', name: 'IMDb', max: 10, votes: true },
      { id: 'metacritic', name: 'Metacritic', max: 100, votes: false },
    ];
    show({
      spec: spec({ value_type: 'score_range', label: 'Score between' }),
      value: { scale: 'imdb', min: null, max: null, min_votes: 500 },
      scales,
      onChange,
    });

    const from = screen.getByLabelText('Score between – from');
    expect(from).toHaveAttribute('max', '10');
    expect(from).toHaveAttribute('step', '0.1');
    await fireEvent.input(from, { target: { value: '7.5' } });
    expect(onChange).toHaveBeenLastCalledWith(
      { scale: 'imdb', min: 7.5, max: null, min_votes: 500 },
      false,
    );
    expect(screen.getByLabelText('Score between – Minimum votes')).toBeTruthy();

    await userEvent.selectOptions(screen.getByLabelText('Score between – Scale'), 'metacritic');
    expect(onChange).toHaveBeenLastCalledWith(
      { scale: 'metacritic', min: null, max: null, min_votes: null },
      false,
    );
  });

  it('steps a hundred-point scale by one, asks no votes of it, and holds a bound it cannot read', async () => {
    const onChange = vi.fn();
    show({
      spec: spec({ value_type: 'score_range', label: 'Score between' }),
      value: { scale: 'metacritic', min: 60, max: null, min_votes: null },
      scales: [
        { id: 'imdb', name: 'IMDb', max: 10, votes: true },
        { id: 'metacritic', name: 'Metacritic', max: 100, votes: false },
      ],
      onChange,
    });

    expect(screen.getByLabelText('Score between – Scale')).toHaveValue('metacritic');
    expect(screen.queryByLabelText('Score between – Minimum votes')).toBeNull();
    const from = screen.getByLabelText('Score between – from');
    expect(from).toHaveAttribute('step', '1');
    await fireEvent.input(from, { target: { value: '1e2' } });
    expect(onChange).toHaveBeenLastCalledWith(
      { scale: 'metacritic', min: 60, max: null, min_votes: null },
      true,
    );
    expect(from).toHaveAttribute('aria-invalid', 'true');
    expect(from).toHaveAccessibleDescription('Enter a number.');
  });

  /** The editor holds Save exactly while a field on screen holds text it could not read. */
  describe('a scale changed under a bound it could not read', () => {
    const scales = [
      { id: 'imdb', name: 'IMDb', max: 10, votes: true },
      { id: 'metacritic', name: 'Metacritic', max: 100, votes: false },
    ];
    const mount = () =>
      renderWithI18n(ConditionValueHarness, {
        props: {
          spec: spec({ value_type: 'score_range', label: 'Score between' }),
          initial: { scale: 'imdb', min: null, max: null, min_votes: null },
          scales,
        },
      });
    const held = () => screen.getByTestId('unreadable').textContent;

    it('forgets a vote count the new scale takes away', async () => {
      mount();
      await fireEvent.input(screen.getByLabelText('Score between – Minimum votes'), {
        target: { value: '1.5' },
      });
      expect(held()).toBe('true');

      await userEvent.selectOptions(screen.getByLabelText('Score between – Scale'), 'metacritic');
      expect(screen.queryByLabelText('Score between – Minimum votes')).toBeNull();
      await fireEvent.input(screen.getByLabelText('Score between – from'), {
        target: { value: '60' },
      });

      expect(held()).toBe('false');
    });

    it('still holds a bound left on screen', async () => {
      mount();
      await fireEvent.input(screen.getByLabelText('Score between – from'), {
        target: { value: '1e2' },
      });

      await userEvent.selectOptions(screen.getByLabelText('Score between – Scale'), 'metacritic');

      expect(held()).toBe('true');
    });
  });

  it('draws a score range with no value before the catalogue lists its scales', () => {
    show({
      spec: spec({ value_type: 'score_range', label: 'Score between' }),
      value: null,
      onChange: vi.fn(),
    });

    expect(screen.getByLabelText('Score between – Scale').querySelectorAll('option')).toHaveLength(
      0,
    );
    expect(screen.getByLabelText('Score between – from')).toHaveValue(null);
    expect(screen.queryByLabelText('Score between – Minimum votes')).toBeNull();
  });

  it('holds a score or a vote count it cannot read, and keeps the votes on a scale that counts them', async () => {
    const onChange = vi.fn();
    const scales = [
      { id: 'imdb', name: 'IMDb', max: 10, votes: true },
      { id: 'tmdb', name: 'TMDB', max: 10, votes: true },
    ];
    show({
      spec: spec({ value_type: 'score_range', label: 'Score between' }),
      value: { scale: 'imdb', min: 7, max: 9, min_votes: 500 },
      scales,
      onChange,
    });

    const votes = screen.getByLabelText('Score between – Minimum votes');
    expect(votes).toHaveAccessibleDescription(
      'Leave empty to take a score whatever its votes. A score from a handful of votes says little.',
    );
    await fireEvent.input(votes, { target: { value: '1.5' } });
    expect(onChange).toHaveBeenLastCalledWith(
      { scale: 'imdb', min: 7, max: 9, min_votes: 500 },
      true,
    );
    expect(votes).toHaveAttribute('aria-invalid', 'true');
    expect(votes).toHaveAccessibleDescription(/Enter a whole number\.$/);

    await fireEvent.input(votes, { target: { value: '1000' } });
    expect(onChange).toHaveBeenLastCalledWith(
      { scale: 'imdb', min: 7, max: 9, min_votes: 1000 },
      false,
    );
    await fireEvent.input(screen.getByLabelText('Score between – to'), {
      target: { value: '' },
    });
    expect(onChange).toHaveBeenLastCalledWith(
      { scale: 'imdb', min: 7, max: null, min_votes: 500 },
      false,
    );

    await userEvent.selectOptions(screen.getByLabelText('Score between – Scale'), 'tmdb');
    expect(onChange).toHaveBeenLastCalledWith(
      { scale: 'tmdb', min: 7, max: 9, min_votes: 500 },
      false,
    );
  });

  it('bounds a runtime range in minutes, not in years', async () => {
    const onChange = vi.fn();
    show({
      spec: spec({ value_type: 'number_range', label: 'Runtime in minutes between' }),
      value: { min: null, max: null },
      onChange,
    });

    const from = screen.getByLabelText('Runtime in minutes between – from');
    expect(from).toHaveAttribute('min', '0');
    expect(from).toHaveAttribute('max', bound('MAX_RUNTIME'));
    await fireEvent.input(from, { target: { value: '40' } });
    expect(onChange).toHaveBeenLastCalledWith({ min: 40, max: null }, false);
  });

  it('names an entry it cannot read as an id, and marks the field', async () => {
    const onChange = vi.fn();
    show({ spec: spec({ value_type: 'number_list', label: 'TMDB id' }), value: [], onChange });
    const field = screen.getByLabelText('TMDB id');

    await fireEvent.input(field, { target: { value: '603, 6O4' } });

    expect(onChange).toHaveBeenLastCalledWith([603], true);
    expect(field).toHaveAttribute('aria-invalid', 'true');
    expect(field).toHaveAccessibleDescription('Not an id, left out until corrected: 6O4');

    await fireEvent.input(field, { target: { value: '603, 604' } });
    expect(onChange).toHaveBeenLastCalledWith([603, 604], false);
    expect(field).not.toHaveAttribute('aria-invalid');
    expect(screen.queryByText(/Not an id/)).toBeNull();
  });

  it('reads an empty year bound as no bound rather than as zero', async () => {
    const onChange = vi.fn();
    show({
      spec: spec({ value_type: 'year_range', label: 'Year between' }),
      value: { min: 1990, max: null },
      onChange,
    });

    await fireEvent.input(screen.getByLabelText('Year between – from'), { target: { value: '' } });

    expect(onChange).toHaveBeenLastCalledWith({ min: null, max: null }, false);
  });

  it('bounds both year fields to what the engine accepts', () => {
    show({ spec: spec({ value_type: 'year_range', label: 'Year between' }), value: {} });

    const ceiling = String(
      new Date().getFullYear() + backendConstant(VALIDATION, 'MAX_YEARS_AHEAD'),
    );
    for (const end of ['from', 'to']) {
      const field = screen.getByLabelText(`Year between – ${end}`);
      expect(field).toHaveAttribute('min', bound('MIN_YEAR'));
      expect(field).toHaveAttribute('max', ceiling);
    }
  });
});
