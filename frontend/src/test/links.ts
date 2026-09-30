/**
 * Click `target` and answer whether a listener took the click, without letting
 * jsdom follow a link nobody took.
 *
 * jsdom implements no navigation to another document. Each one it is asked for
 * prints "not implemented" into the run, after the test that asked, where
 * nothing names the test. A test that leaves a link to the browser reads here
 * that it did, and the browser's part is stopped at the window, past every
 * listener the application installs.
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
