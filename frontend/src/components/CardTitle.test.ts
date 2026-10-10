import { describe, it, expect, afterEach, vi } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { CARD_HELP } from '../lib/help';
import CardTitleHarness from '../test/CardTitleHarness.svelte';

/**
 * A card's title carries a "?" named after the card, which opens a few
 * sentences on what the card shows. The browser opens and places the popover,
 * which jsdom does not lay out: what is held here is the name, the link from
 * the button to its bubble, and the text written in once it opens.
 */

const card = Object.keys(CARD_HELP)[0]!;
const STRINGS = {
  HelpOn: 'Help:',
  [CARD_HELP[card]!.text]: 'What this card shows.',
};

/** The browser's `toggle` event, which jsdom cannot fire for a popover it does not show. */
async function toggle(bubble: Element, newState: 'open' | 'closed') {
  const event = new Event('toggle');
  Object.assign(event, { newState });
  await fireEvent(bubble, event);
}

describe('CardTitle', () => {
  it('names its "?" after the card it explains', () => {
    renderWithI18n(CardTitleHarness, { props: { card, title: 'Backups' }, strings: STRINGS });

    expect(screen.getByRole('heading', { level: 2, name: 'Backups' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Help: Backups' })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
  });

  it('writes its text into the bubble once it opens, and takes it back on closing', async () => {
    renderWithI18n(CardTitleHarness, { props: { card, title: 'Backups' }, strings: STRINGS });
    const button = screen.getByRole('button', { name: 'Help: Backups' });
    const bubble = document.getElementById(button.getAttribute('popovertarget')!)!;

    expect(bubble).toHaveAttribute('popover', 'auto');
    expect(bubble.textContent?.trim()).toBe('');

    await toggle(bubble, 'open');
    expect(bubble).toHaveTextContent('What this card shows.');
    expect(button).toHaveAttribute('aria-expanded', 'true');

    await toggle(bubble, 'closed');
    expect(bubble.textContent?.trim()).toBe('');
    expect(button).toHaveAttribute('aria-expanded', 'false');
  });

  it('keeps an id something else points at', () => {
    renderWithI18n(CardTitleHarness, {
      props: { card, title: 'Getting started', id: 'guide-title' },
      strings: STRINGS,
    });

    expect(screen.getByRole('heading', { name: 'Getting started' })).toHaveAttribute(
      'id',
      'guide-title',
    );
    expect(screen.getByRole('button', { name: 'Help: Getting started' })).toBeInTheDocument();
  });

  it('draws a title of the level its card asks for', () => {
    renderWithI18n(CardTitleHarness, {
      props: { card, title: 'Metadata', level: 3 },
      strings: STRINGS,
    });

    expect(screen.getByRole('heading', { level: 3, name: 'Metadata' })).toBeInTheDocument();
  });
});

/**
 * The bubble opens under its "?", or above it when the window ends first, and
 * stays inside the window. jsdom lays nothing out, so the boxes are given.
 */
describe('the bubble of a card title', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.documentElement.dir = '';
  });

  function boxes(anchor: Partial<DOMRect>, bubble: Partial<DOMRect>) {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
      this: HTMLElement,
    ) {
      const box = this.classList.contains('help-bubble') ? bubble : anchor;
      return { top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0, ...box } as DOMRect;
    });
  }

  async function opened() {
    renderWithI18n(CardTitleHarness, { props: { card, title: 'Backups' }, strings: STRINGS });
    const button = screen.getByRole('button', { name: 'Help: Backups' });
    const bubble = document.getElementById(button.getAttribute('popovertarget')!)!;
    await toggle(bubble, 'open');
    return bubble;
  }

  it('opens under its "?", where the window leaves it room', async () => {
    boxes({ top: 100, bottom: 124, left: 40, right: 64 }, { width: 200, height: 80 });
    const bubble = await opened();

    expect(bubble.style.top).toBe('130px');
    expect(bubble.style.left).toBe('40px');
    expect(bubble).toHaveClass('is-placed');
  });

  it('opens above its "?" near the bottom of the window, and inside its right edge', async () => {
    boxes(
      {
        top: innerHeight - 30,
        bottom: innerHeight - 6,
        left: innerWidth - 20,
        right: innerWidth - 4,
      },
      { width: 200, height: 80 },
    );
    const bubble = await opened();

    expect(bubble.style.top).toBe(`${innerHeight - 30 - 6 - 80}px`);
    expect(bubble.style.left).toBe(`${innerWidth - 200 - 8}px`);
  });

  it('lines up with the end of its "?" on a page written right to left', async () => {
    document.documentElement.dir = 'rtl';
    boxes({ top: 100, bottom: 124, left: 300, right: 324 }, { width: 200, height: 80 });
    const bubble = await opened();

    expect(bubble.style.left).toBe('124px');
  });

  it('follows its "?" when the page scrolls under it', async () => {
    boxes({ top: 100, bottom: 124, left: 40, right: 64 }, { width: 200, height: 80 });
    const bubble = await opened();
    boxes({ top: 50, bottom: 74, left: 40, right: 64 }, { width: 200, height: 80 });

    await fireEvent.scroll(window);
    expect(bubble.style.top).toBe('80px');
  });
});
