import { describe, it, expect, vi } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
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
  PickerCountCaption: 'Titles in your library',
};

const OPTIONS = [
  { value: 'Animation', count: 27 },
  { value: 'Science Fiction', count: 12 },
  { value: 'Comédie', count: 4 },
];

/**
 * A closed vocabulary: the value a rule stores is a code, and the label is the
 * only part a reader recognises.
 */
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
  /**
   * Certification codes of several countries mean the same thing, and a rule
   * wants every one of them: listed under what they mean, "TP" and "U" read
   * as the same choice instead of two unexplained entries.
   */
  it('gathers the values that mean the same thing under that meaning', async () => {
    const { field } = open([], {
      options: [
        { value: 'TP', label: 'TP (all ages)', group: 'all ages', count: 24 },
        { value: 'U', label: 'U (all ages)', group: 'all ages', count: 1 },
        { value: '12', label: '12 (12 and over)', group: '12 and over', count: 11 },
        { value: 'M', count: 1 },
      ],
    });

    await userEvent.click(field);

    const allAges = screen.getByRole('group', { name: 'all ages' });
    expect(
      within(allAges)
        .getAllByRole('option')
        .map((o) => o.getAttribute('aria-label')),
    ).toEqual(['TP (all ages)', 'U (all ages)']);
    // Under its meaning a code needs no repeat of it.
    expect(within(allAges).getByText('TP')).toBeTruthy();
    expect(
      within(screen.getByRole('group', { name: '12 and over' })).getAllByRole('option'),
    ).toHaveLength(1);
    expect(screen.getByRole('option', { name: 'M' }).closest('[role="group"]')).toBeNull();
  });

  it('says what the figures beside the values count', async () => {
    const { field } = open();

    await userEvent.click(field);

    expect(screen.getByText('Titles in your library')).toBeTruthy();
  });

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

  /**
   * Inside the rule editor's dialog, an Escape nobody claims closes the dialog
   * and throws the draft away. Folding the list is all it may do here.
   */
  it('folds the list on Escape and claims the key, then lets the next Escape through', async () => {
    const { field } = open();
    await userEvent.click(field);

    expect(await fireEvent.keyDown(field, { key: 'Escape' })).toBe(false);
    expect(field).toHaveAttribute('aria-expanded', 'false');
    expect(await fireEvent.keyDown(field, { key: 'Escape' })).toBe(true);
  });

  /** Nothing typed and nothing chosen: Enter is the form's, which saves. */
  it('adds nothing on Enter in an empty picker, and lets the form have it', async () => {
    const { onChange, field } = open();
    await userEvent.click(field);

    expect(await fireEvent.keyDown(field, { key: 'Enter' })).toBe(true);
    expect(onChange).not.toHaveBeenCalled();
  });

  /** The focus stays in the field and the arrows move through the list. */
  it('adds the suggestion the arrows reach, and keeps the focus in the field', async () => {
    const { onChange, field } = open();
    await userEvent.click(field);

    await userEvent.keyboard('{ArrowDown}{ArrowDown}');
    const reached = screen.getByRole('option', { name: 'Science Fiction' });
    expect(reached).toHaveAttribute('aria-selected', 'true');
    expect(field).toHaveAttribute('aria-activedescendant', reached.id);
    await userEvent.keyboard('{Enter}');

    expect(onChange).toHaveBeenCalledWith(['Science Fiction']);
    expect(document.activeElement).toBe(field);
  });

  /** Fifty suggestions are not fifty tab stops before the next control. */
  it('is one tab stop, whatever the list holds', async () => {
    const { field } = open();
    await userEvent.click(field);
    expect(screen.getAllByRole('option').every((option) => option.tabIndex < 0)).toBe(true);

    await userEvent.tab();

    expect(document.activeElement?.getAttribute('role')).not.toBe('option');
  });

  it('keeps the focus in the field when a suggestion is clicked', async () => {
    const { onChange, field } = open();
    await userEvent.click(field);

    await userEvent.click(screen.getByRole('option', { name: 'Animation' }));

    expect(onChange).toHaveBeenCalledWith(['Animation']);
    expect(document.activeElement).toBe(field);
  });

  /** Typed again, a value already chosen offers nothing to add either. */
  it('says so when the filter matches nothing it could still add', async () => {
    const { field } = open(['zzz']);
    await userEvent.type(field, 'zzz');

    expect(screen.getByText('No value matches')).toBeInTheDocument();
    expect(screen.queryByRole('option', { name: /zzz/ })).toBeNull();
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

  /** A combobox announces itself, its state and the list it drives. */
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
   * A vocabulary entry the library does not carry is offered all the same
   * (that is what lets a rule be written before the first Korean film arrives),
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

/**
 * A chip's button goes with its chip, and the focus with it, inside the rule
 * editor's dialog. It moves to the chip that takes its place,
 * else to the one before, else to the field, so removing several values is
 * one key pressed several times.
 */
describe('removing a chosen value', () => {
  function openLive(values: string[]) {
    const view = renderWithI18n(ValuePicker, {
      props: {
        label: 'Genre contains',
        values,
        options: OPTIONS,
        onChange: (next: string[]) => void view.rerender({ values: next }),
      },
      strings: STRINGS,
    });
  }

  it('hands the focus to the chip that took its place', async () => {
    openLive(['Animation', 'Comédie']);

    await userEvent.click(screen.getByRole('button', { name: 'Remove – Animation' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove – Comédie' })),
    );
  });

  it('hands the focus to the chip before a last one removed', async () => {
    openLive(['Animation', 'Comédie']);

    await userEvent.click(screen.getByRole('button', { name: 'Remove – Comédie' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Remove – Animation' }),
      ),
    );
  });

  it('hands the focus to the field once no value is left', async () => {
    openLive(['Animation']);

    await userEvent.click(screen.getByRole('button', { name: 'Remove – Animation' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('combobox', { name: 'Genre contains' })),
    );
  });
});
