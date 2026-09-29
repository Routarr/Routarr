import { tick } from 'svelte';

/** An element, or the id of one, that may be gone by the time it is wanted. */
export type FocusTarget = HTMLElement | string | null | undefined;

/**
 * Hands the focus on once the screen has redrawn, when the control that held
 * it went with the redraw.
 *
 * A focused button that leaves the page with its row, or turns disabled while
 * its action runs, drops the focus to `<body>`, where a screen reader loses its
 * place. The first target still on the page and able to take it gets it. A
 * focus that is still somewhere, as in the field a save by Enter came from,
 * stays where it is.
 */
export async function handFocus(...targets: FocusTarget[]): Promise<void> {
  await tick();
  const active = document.activeElement;
  if (active && active !== document.body && !active.matches(':disabled')) return;
  for (const target of targets) {
    const element = typeof target === 'string' ? document.getElementById(target) : target;
    if (element?.isConnected && !element.matches(':disabled')) {
      element.focus();
      return;
    }
  }
}

/**
 * Move the focus to the heading of the screen `container` shows, as a page
 * load would, so a screen reader announces where a link led.
 *
 * A screen is its own chunk and renders once that loads, so the heading is
 * waited for rather than looked up once. Returns what stops the wait.
 */
export function focusHeadingOf(container: HTMLElement): () => void {
  const focus = (): boolean => {
    const heading = container.querySelector<HTMLElement>('h1');
    if (!heading) return false;
    heading.tabIndex = -1;
    heading.focus({ preventScroll: true });
    return true;
  };
  if (focus()) return () => {};
  const observer = new MutationObserver(() => {
    if (focus()) observer.disconnect();
  });
  observer.observe(container, { childList: true, subtree: true });
  return () => observer.disconnect();
}
