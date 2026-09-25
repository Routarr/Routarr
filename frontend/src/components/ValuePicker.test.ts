import { describe, it, expect, vi } from 'vitest';
import { screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import ValuePicker from './ValuePicker.svelte';

/**
 * Choosing a condition's values from what the library holds.
 *
 * The values are the strings the engine compares, so the control exists to stop
 * a rule depending on the user knowing an exact spelling. What it must never do
 * is store a value twice under two spellings, or lose one already saved.
 */

const STRINGS = {
  AddValue: 'Add "{value}"',
  Loading: 'Loading…',
  NoMatchingValue: 'No value matches',
  NoValueInLibrary: 'The library carries no value for this yet',
  PlaceholderPickValues: 'Search or type a value…',
  Remove: 'Remove',
  SelectedValues: 'Selected values',
};

const OPTIONS = [
  { value: 'Animation', count: 27 },
  { value: 'Science Fiction', count: 12 },
  { value: 'Comédie', count: 4 },
];

/// A closed vocabulary: the value a rule stores is a code, and the label is the
/// only part a reader recognises.
const CODES = [
  { value: 'ja', label: 'Japanese (ja)', count: 3 },
  { value: 'ko', label: 'Korean (ko)', count: 0 },
];

function open(values: string[] = [], over: Record<string, unknown> = {}) {
  const onChange = vi.fn();
  renderWithI18n(ValuePicker, {
    props: { label: 'Genre contains', values, options: OPTIONS, onChange, ...over },
    strings: STRINGS,
  });
  return { onChange, field: screen.getByLabelText('Genre contains') };
}

describe('ValuePicker', () => {
  it('offers what the library holds, with how many items carry it', async () => {
    const { field } = open();
    await userEvent.click(field);

    expect(screen.getByRole('option', { name: 'Animation' })).toBeInTheDocument();
    expect(screen.getByText('27')).toBeInTheDocument();
  });

  it('adds the value the option carries, not the text that was typed', async () => {
    const { onChange, field } = open();
    // Lower case and no accent: the user should not have to know either.
    await userEvent.type(field, 'comedie');
    await userEvent.click(screen.getByRole('option', { name: 'Comédie' }));

    expect(onChange).toHaveBeenCalledWith(['Comédie']);
  });

  /**
   * The filter matches on the same canonical form the engine compares with, so
   * a search that ignores case, accents and separators finds the value anyway.
   */
  it('finds a value however it is spelled', async () => {
    const { field } = open();
    await userEvent.type(field, 'science-fiction');

    expect(screen.getByRole('option', { name: 'Science Fiction' })).toBeInTheDocument();
  });

  it('renders the values already stored, and gives each one a way out', async () => {
    const { onChange } = open(['Animation', 'Comédie']);

    expect(screen.getByLabelText('Selected values')).toBeInTheDocument();
    await userEvent.click(screen.getByLabelText('Remove – Animation'));

    expect(onChange).toHaveBeenCalledWith(['Comédie']);
  });

  /**
   * A value already chosen is not offered again, and cannot be added a second
   * time under another spelling: two chips the engine treats as one would make
   * the rule read as narrower than it is.
   */
  it('never offers or stores a value twice', async () => {
    const { field } = open(['Animation']);
    await userEvent.click(field);

    expect(screen.queryByRole('option', { name: 'Animation' })).not.toBeInTheDocument();

    await userEvent.type(field, 'animation');
    expect(screen.queryByText('Add "animation"')).not.toBeInTheDocument();
  });

  it('accepts a value the library does not carry yet', async () => {
    const { onChange, field } = open();
    await userEvent.type(field, 'Mockumentary');
    await userEvent.click(screen.getByRole('option', { name: /Mockumentary/ }));

    expect(onChange).toHaveBeenCalledWith(['Mockumentary']);
  });

  it('commits on Enter, so the list is usable from the keyboard alone', async () => {
    const { onChange, field } = open();
    await userEvent.type(field, 'anim{Enter}');

    expect(onChange).toHaveBeenCalledWith(['Animation']);
  });

  it('says so when the filter matches nothing', async () => {
    const { field } = open();
    await userEvent.type(field, 'zzz');

    // The typed value is still offerable, and the list says the library has
    // nothing like it rather than rendering an empty box.
    expect(screen.getByText('Add "zzz"')).toBeInTheDocument();
  });

  it('says so when the library carries nothing at all', async () => {
    const { field } = open([], { options: [] });
    await userEvent.click(field);

    expect(screen.getByText('The library carries no value for this yet')).toBeInTheDocument();
  });

  it('says it is still loading rather than claiming the library is empty', async () => {
    const { field } = open([], { loading: true });
    await userEvent.click(field);

    expect(screen.getByText('Loading…')).toBeInTheDocument();
    expect(screen.queryByText('The library carries no value for this yet')).not.toBeInTheDocument();
  });

  it('carries the failure rather than a blank list', async () => {
    const { field } = open([], { error: 'Library unreachable' });
    await userEvent.click(field);

    expect(screen.getByText('Library unreachable')).toBeInTheDocument();
  });

  /// A combobox announces itself, its state and the list it drives.
  it('is announced as a combobox tied to its own list', async () => {
    const { field } = open();

    expect(field).toHaveAttribute('role', 'combobox');
    expect(field).toHaveAttribute('aria-expanded', 'false');

    // The id is this instance's own, so two pickers on one form do not share it.
    await userEvent.click(field);
    const list = screen.getByRole('listbox');
    expect(list.id).toBeTruthy();
    expect(field).toHaveAttribute('aria-controls', list.id);
  });

  /**
   * A language is stored as `ja` and read as "Japanese". Searching by the name,
   * storing the code, and showing the name back is the whole point: neither
   * half is guessable from the other.
   */
  it('searches a coded value by its name and stores the code', async () => {
    const { onChange, field } = open([], { options: CODES });
    await userEvent.type(field, 'korean');
    await userEvent.click(screen.getByRole('option', { name: 'Korean (ko)' }));

    expect(onChange).toHaveBeenCalledWith(['ko']);
  });

  it('shows a stored code under its name', () => {
    open(['ja'], { options: CODES });

    expect(screen.getByText('Japanese (ja)')).toBeInTheDocument();
    expect(screen.getByLabelText('Remove – Japanese (ja)')).toBeInTheDocument();
  });

  /**
   * A vocabulary entry the library does not carry is offered all the same —
   * that is what lets a rule be written before the first Korean film arrives —
   * and it carries no figure, which would otherwise read as "absent".
   */
  it('offers a value the library has never seen, without a count', async () => {
    const { field } = open([], { options: CODES });
    await userEvent.click(field);

    const korean = screen.getByRole('option', { name: 'Korean (ko)' });
    expect(korean).toBeInTheDocument();
    expect(korean.textContent).not.toContain('0');
    expect(screen.getByRole('option', { name: 'Japanese (ja)' }).textContent).toContain('3');
  });
});
