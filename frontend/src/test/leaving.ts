/**
 * Whether a reload or a closed tab would be held: what the page's
 * `beforeunload` listeners answer, the one question a browser lets a page ask
 * there.
 */
export function unloading(): boolean {
  const event = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(event);
  return event.defaultPrevented;
}
