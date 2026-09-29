import { describe, it, expect, afterEach } from 'vitest';

import { focusHeadingOf, handFocus } from './focus';

function button(id: string, disabled = false): HTMLButtonElement {
  const element = document.createElement('button');
  element.id = id;
  element.disabled = disabled;
  document.body.append(element);
  return element;
}

afterEach(() => document.body.replaceChildren());

describe('handFocus', () => {
  it('gives the focus to the first target still there and able to take it', async () => {
    button('held', true);
    const detached = document.createElement('button');
    const next = button('next');

    await handFocus('gone', 'held', detached, next);

    expect(document.activeElement).toBe(next);
  });

  /**
   * A browser drops the focus of a control that turns disabled, and a test DOM
   * may keep it there: either way nothing can be done from it.
   */
  it('takes the focus back from a control left disabled', async () => {
    const pressed = button('pressed');
    pressed.focus();
    pressed.disabled = true;
    const neighbour = button('neighbour');

    await handFocus(neighbour);

    expect(document.activeElement).toBe(neighbour);
  });
});

describe('focusHeadingOf', () => {
  it('focuses a heading already on screen', () => {
    const container = document.createElement('div');
    container.innerHTML = '<h1>Rules</h1>';
    document.body.append(container);

    focusHeadingOf(container);

    expect(document.activeElement?.textContent).toBe('Rules');
  });

  /** A screen renders once its chunk loads, after the navigation. */
  it('waits for a heading that renders later', async () => {
    const container = document.createElement('div');
    document.body.append(container);

    focusHeadingOf(container);
    expect(document.activeElement).toBe(document.body);
    container.innerHTML = '<section><h1>Logs</h1></section>';

    await Promise.resolve();
    expect(document.activeElement?.textContent).toBe('Logs');
  });

  it('stops waiting once told to', async () => {
    const container = document.createElement('div');
    document.body.append(container);

    focusHeadingOf(container)();
    container.innerHTML = '<h1>Logs</h1>';

    await Promise.resolve();
    expect(document.activeElement).toBe(document.body);
  });
});
