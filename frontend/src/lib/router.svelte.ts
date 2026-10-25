import { basePath } from '../api/basePath';

/**
 * The whole router: flat routes, no nesting, no parameters. Written here because the mount
 * point is read at run time from the `<base href>` the backend injects, where a router library
 * wants it at build time, one image per mount point.
 */

// Without the mount point and without a trailing slash: a bookmark or a proxy
// may add one, and `/rules/` is the Rules screen, not a page that is not found.
const strip = (pathname: string) => {
  const base = basePath();
  const under = base && (pathname === base || pathname.startsWith(`${base}/`));
  const path = under ? pathname.slice(base.length) : pathname;
  const rooted = path.startsWith('/') ? path : `/${path}`;
  return rooted.length > 1 ? rooted.replace(/\/+$/, '') : rooted;
};

/** The current route, without the mount point. Reactive. */
export const router = $state({ path: strip(window.location.pathname) });

// Arrived with a trailing slash, the address says the screen as its links do.
if (router.path !== '/' && window.location.pathname.endsWith('/')) {
  const { search, hash } = window.location;
  window.history.replaceState(
    window.history.state,
    '',
    `${basePath()}${router.path}${search}${hash}`,
  );
}

/**
 * The `href` of a route: the mount point and the path. A `<base href>` applies to relative
 * URLs only, so a bare `/rules` would send a middle-click or a copied address to the proxy's root.
 */
export const href = (to: string) => `${basePath()}${to}`;

/**
 * Asked before the screen changes, by a screen holding work the change would
 * drop. One at a time: only the screen on display can hold any. `to` is the
 * route without its fragment, and a change within the screen asks nothing.
 */
type LeaveGuard = (to: string) => Promise<boolean>;
let guard: LeaveGuard | null = null;

/** Install `check` until the returned function is called. */
export function guardLeaving(check: LeaveGuard): () => void {
  guard = check;
  return () => {
    if (guard === check) guard = null;
  };
}

const routeOf = (to: string) => to.split(/[?#]/)[0] || '/';

// The address of the screen on display. The browser has moved by the time Back
// is heard, so a screen that keeps its work is put back under this one.
let shown = window.location.href;

/**
 * Follow a link the way the browser would, without reloading the document,
 * once a screen holding unsaved work has let it go.
 */
export function navigate(to: string) {
  if (guard && routeOf(to) !== router.path) {
    void guard(routeOf(to)).then((leave) => {
      if (leave) go(to);
    });
    return;
  }
  go(to);
}

function go(to: string) {
  const { pathname, search, hash, href: from } = window.location;
  const url = `${basePath()}${to}`;
  // The page on screen takes no second entry, as with a link in the browser.
  const here = url === `${pathname}${search}${hash}`;
  if (here) window.history.replaceState({}, '', url);
  else window.history.pushState({}, '', url);
  router.path = strip(window.location.pathname);
  // A link to another fragment of the page on screen is one the browser
  // follows with a `hashchange`, which `pushState` never fires. A page that
  // shows a section per fragment, as Settings does, listens for it.
  if (pathname === window.location.pathname && hash !== window.location.hash) {
    window.dispatchEvent(
      new HashChangeEvent('hashchange', { oldURL: from, newURL: window.location.href }),
    );
  }
  shown = window.location.href;
  window.scrollTo(0, 0);
}

/**
 * One delegated listener, so the markup stays a plain `<a href>`: middle-click, the status bar
 * and a screen reader all treat it as the link it is.
 */
export function interceptLinks() {
  const onClick = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;

    const anchor = (event.target as Element | null)?.closest?.('a');
    if (!anchor) return;

    const target = anchor.getAttribute('href');
    if (!target || anchor.target || anchor.hasAttribute('download')) return;
    // Absolute, protocol-relative or external: let the browser have it.
    if (!target.startsWith('/') || target.startsWith('//')) return;
    if (anchor.getAttribute('rel')?.includes('external')) return;
    // Another application on the same host, beside this mount point.
    const base = basePath();
    if (base && target !== base && !target.startsWith(`${base}/`)) return;
    // The server answers anything under `/api`, the sign-in with an identity
    // provider included. Taken here, it changes the address and nothing else.
    const route = strip(target);
    if (route === '/api' || route.startsWith('/api/')) return;

    event.preventDefault();
    navigate(route);
  };

  const onPop = () => {
    const to = strip(window.location.pathname);
    const back = shown;
    if (!guard || to === router.path) {
      router.path = to;
      shown = window.location.href;
      return;
    }
    void guard(to).then((leave) => {
      if (leave) {
        router.path = strip(window.location.pathname);
        shown = window.location.href;
      } else {
        window.history.pushState({}, '', back);
      }
    });
  };

  document.addEventListener('click', onClick);
  window.addEventListener('popstate', onPop);

  return () => {
    document.removeEventListener('click', onClick);
    window.removeEventListener('popstate', onPop);
  };
}

/**
 * Whether a navigation entry is the one being shown: its own path, or one
 * under it. Compared up to a separator, so `/rules` stays dark on `/rule-tests`.
 */
export function isCurrent(path: string): boolean {
  return router.path === path || router.path.startsWith(`${path}/`);
}
