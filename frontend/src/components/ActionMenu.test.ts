import { describe, it, expect, vi } from 'vitest';
import { fireEvent, screen, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import ActionMenu from './ActionMenu.svelte';

const show = (actions: unknown[]) => renderWithI18n(ActionMenu, { props: { actions } });

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

  /** A dialog the action opens hands the focus back to whatever held it when it opened. */
  it('hands the focus to its trigger before the action runs', async () => {
    let focusedWhenRun: Element | null = null;
    show([{ label: 'Sync now', onSelect: () => (focusedWhenRun = document.activeElement) }]);
    const trigger = screen.getByRole('button', { name: 'Actions' });

    await fireEvent.click(trigger);
    await fireEvent.click(screen.getByRole('menuitem', { name: 'Sync now' }));

    expect(focusedWhenRun).toBe(trigger);
  });

  it('closes on Escape and hands the focus back to its trigger', async () => {
    show([{ label: 'Sync now', onSelect: vi.fn() }]);
    const trigger = screen.getByRole('button', { name: 'Actions' });
    await fireEvent.click(trigger);

    await fireEvent.keyDown(document, { key: 'Escape' });

    expect(screen.queryByRole('menuitem')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  /** `role="menu"` tells a screen reader user to reach for the arrows. */
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

  it('walks the items of every group as one list', async () => {
    show([
      { label: 'Genre contains', group: 'From the metadata sources', onSelect: vi.fn() },
      { label: 'Season count over', group: 'From Radarr and Sonarr', onSelect: vi.fn() },
    ]);
    await fireEvent.click(screen.getByRole('button', { name: 'Actions' }));

    const arr = screen.getByRole('group', { name: 'From Radarr and Sonarr' });
    await fireEvent.keyDown(screen.getByRole('menu'), { key: 'ArrowDown' });

    expect(document.activeElement).toBe(within(arr).getByRole('menuitem'));
  });

  it('closes when the click lands anywhere else', async () => {
    show([{ label: 'Sync now', onSelect: vi.fn() }]);
    await fireEvent.click(screen.getByRole('button', { name: 'Actions' }));

    await fireEvent.mouseDown(document.body);

    expect(screen.queryByRole('menuitem')).toBeNull();
  });

  /** A trigger that opens onto nothing reads as a broken control. */
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
