import { untrack } from 'svelte';
import { router } from './router.svelte';

/** One filter of the address, `''` when it is absent or names nothing in `allowed`. */
export type ReadFilter = (name: string, allowed?: readonly string[]) => string;

const read: ReadFilter = (name, allowed) => {
  const value = new URLSearchParams(window.location.search).get(name) ?? '';
  return allowed && !allowed.includes(value) ? '' : value;
};

/** The query of the address with `values` written in it, an empty one left out. */
function queryWith(values: Record<string, string>): string {
  const others = [...new URLSearchParams(window.location.search)].filter(
    ([name]) => !(name in values),
  );
  const set = Object.entries(values).filter(([, value]) => value);
  const query = new URLSearchParams([...others, ...set]).toString();
  return query ? `?${query}` : '';
}

/**
 * Keeps a list's filters in the address, so Back, a reload and a shared link find them again.
 * `take` sets them from the address on arrival, and again when a link changes the query of the
 * screen on display. Opens effects, so it is called while a component initialises.
 */
export function keepInAddress(
  shown: () => Record<string, string>,
  take: (read: ReadFilter) => void,
): void {
  take(read);
  let visit = router.visit;
  $effect(() => {
    if (router.visit === visit) return;
    visit = router.visit;
    untrack(() => take(read));
  });

  // In place, so Back leaves the screen rather than undoing a filter.
  const pathname = window.location.pathname;
  $effect(() => {
    const query = queryWith(shown());
    if (window.location.pathname !== pathname || query === window.location.search) return;
    window.history.replaceState(
      window.history.state,
      '',
      `${pathname}${query}${window.location.hash}`,
    );
  });
}
