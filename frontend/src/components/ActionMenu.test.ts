import { describe, it, expect, vi } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import ActionMenu from './ActionMenu.svelte';

/**
 * The overflow menu exists because seven buttons in one row run past a 1440px
 * screen, and the ones past the edge go off it with nothing saying so.
 */

const STRINGS = { Actions: 'Actions' };

const show = (actions: unknown[]) =>
  renderWithI18n(ActionMenu, { props: { actions }, strings: STRINGS });

describe('ActionMenu', () => {
  it('keeps its items out of the tab order until it is opened', async () => {
    show([{ label: 'Sync now', onSelect: vi.fn() }]);

    expect(screen.queryByRole('menuitem')).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Actions' }));

    expect(screen.getByRole('menuitem', { name: 'Sync now' })).toBeTruthy();
  });

  it('says whether it is open, rather than only looking open', async () => {
    show([{ label: 'Sync now', onSelect: vi.fn() }]);
    const trigger = screen.getByRole('button', { name: 'Actions' });

    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    expect(trigger.getAttribute('aria-haspopup')).toBe('menu');

    await fireEvent.click(trigger);
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
  });

  it('runs the action and closes, so the menu is never left hanging open', async () => {
    const sync = vi.fn();
    show([{ label: 'Sync now', onSelect: sync }]);

    await fireEvent.click(screen.getByRole('button', { name: 'Actions' }));
    await fireEvent.click(screen.getByRole('menuitem', { name: 'Sync now' }));

    expect(sync).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('menuitem')).toBeNull();
  });

  it('closes on Escape and hands the focus back to its trigger', async () => {
    show([{ label: 'Sync now', onSelect: vi.fn() }]);
    const trigger = screen.getByRole('button', { name: 'Actions' });
    await fireEvent.click(trigger);

    await fireEvent.keyDown(document, { key: 'Escape' });

    expect(screen.queryByRole('menuitem')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  /**
   * `role="menu"` announces a menu, and a screen reader user then reaches for
   * the arrows: a menu that only answered clicks was announced as something it
   * was not.
   */
  it('moves the focus in on opening and walks the items with the arrows', async () => {
    show([
      { label: 'Sync now', onSelect: vi.fn() },
      { label: 'Test connection', onSelect: vi.fn() },
      { label: 'Delete', onSelect: vi.fn() },
    ]);
    await fireEvent.click(screen.getByRole('button', { name: 'Actions' }));

    const item = (name: string) => screen.getByRole('menuitem', { name });
    expect(document.activeElement).toBe(item('Sync now'));

    const menu = screen.getByRole('menu');
    await fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(item('Test connection'));
    await fireEvent.keyDown(menu, { key: 'End' });
    expect(document.activeElement).toBe(item('Delete'));
    await fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(item('Sync now'));
    await fireEvent.keyDown(menu, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(item('Delete'));

    // One tab stop: the items themselves are reached by arrow, not by Tab.
    for (const name of ['Sync now', 'Test connection', 'Delete']) {
      expect(item(name).getAttribute('tabindex')).toBe('-1');
    }
  });

  it('closes when the click lands anywhere else', async () => {
    show([{ label: 'Sync now', onSelect: vi.fn() }]);
    await fireEvent.click(screen.getByRole('button', { name: 'Actions' }));

    await fireEvent.mouseDown(document.body);

    expect(screen.queryByRole('menuitem')).toBeNull();
  });

  /**
   * A trigger that opens onto nothing is worse than no trigger: it reads as a
   * broken control rather than an absent one.
   */
  it('renders nothing at all when every action is disabled', () => {
    const { container } = show([
      { label: 'Sync now', disabled: true, onSelect: vi.fn() },
      { label: 'Delete', disabled: true, onSelect: vi.fn() },
    ]);

    expect(container.querySelector('.action-menu')).toBeNull();
  });

  it('offers only the actions that are usable', async () => {
    show([
      { label: 'Sync now', onSelect: vi.fn() },
      { label: 'Delete', disabled: true, onSelect: vi.fn() },
    ]);

    await fireEvent.click(screen.getByRole('button', { name: 'Actions' }));

    expect(screen.getAllByRole('menuitem')).toHaveLength(1);
    expect(screen.queryByRole('menuitem', { name: 'Delete' })).toBeNull();
  });
});
