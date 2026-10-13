/**
 * Clicks `target` and answers whether a listener took the click, without letting jsdom
 * follow the link.
 *
 * jsdom implements no navigation and prints "not implemented" after the test that asked, where
 * nothing names it. The default is prevented at the window, past every application listener.
 */
export function click(target: Element, init: MouseEventInit = {}): boolean {
  let taken = false;
  const stay = (event: Event) => {
    taken = event.defaultPrevented;
    event.preventDefault();
  };
  window.addEventListener('click', stay);
  try {
    target.dispatchEvent(
      new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, ...init }),
    );
  } finally {
    window.removeEventListener('click', stay);
  }
  return taken;
}
