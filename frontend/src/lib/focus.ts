import { tick } from 'svelte';

/** An element, or the id of one, that may be gone by the time it is wanted. */
export type FocusTarget = HTMLElement | string | null | undefined;

/**
 * Once the screen has redrawn, hands the focus to the first target still on the page and
 * enabled, when the control that held it is gone or disabled.
 *
 * A control that goes or turns disabled drops the focus to `<body>`, where a screen reader
 * loses its place. A focus held elsewhere, as in the field a save by Enter came from, stays.
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
 * Moves the focus to the heading of the screen `container` shows, so a screen reader
 * announces where a link led.
 *
 * The heading may render later, once the screen's chunk loads, so it is waited for. Returns
 * what stops the wait.
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
