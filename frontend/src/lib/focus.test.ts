import { describe, it, expect, afterEach } from 'vitest';

import { handFocus } from './focus';

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
