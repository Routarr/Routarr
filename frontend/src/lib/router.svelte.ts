import { basePath } from '../api/basePath';

/**
 * The whole router. Flat routes, no nesting, no parameters.
 *
 * Written rather than installed, for one reason that is specific to this
 * application: the mount point is discovered at *runtime* from the `<base href>`
 * the backend injects, because a single Docker image has to serve any sub-path.
 * Every SPA router worth taking wants its base at build time, which would mean
 * one image per mount point, the opposite of what this project ships.
 *
 * A router this small also costs less than tracking a routing dependency's
 * position on Svelte 5.
 */

const strip = (pathname: string) => {
  const base = basePath();
  const under = base && (pathname === base || pathname.startsWith(`${base}/`));
  const path = under ? pathname.slice(base.length) : pathname;
  return path.startsWith('/') ? path : `/${path}`;
};

/** The current route, without the mount point. Reactive. */
export const router = $state({ path: strip(window.location.pathname) });

/**
 * The `href` of a route: the mount point and the path.
 *
 * A `<base href>` applies to relative URLs only, and these anchors are
 * root-absolute. A bare `/rules` lies outside the mount point, so the
 * interception below leaves it to the browser, and every way of following it
 * (a click, middle-click, ctrl-click, a copied address) reaches the proxy's
 * root and a 404. Every anchor to a route goes through this, so the attribute
 * says where the click will actually go.
 */
export const href = (to: string) => `${basePath()}${to}`;

/** Follow a link the way the browser would, without reloading the document. */
export function navigate(to: string, options: { replace?: boolean } = {}) {
  const { pathname, search, hash, href: from } = window.location;
  const url = `${basePath()}${to}`;
  // The page on screen takes no second entry, as with a link in the browser.
  const here = url === `${pathname}${search}${hash}`;
  if (options.replace || here) window.history.replaceState({}, '', url);
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
  window.scrollTo(0, 0);
}

/**
 * One delegated listener rather than a `<Link>` component.
 *
 * The markup stays a plain `<a href>`, which is what makes a link a link:
 * middle-click opens a tab, ctrl-click too, the status bar shows a destination,
 * and a screen reader announces it as a link with a name. A component that
 * renders a `<div role="link">` gets none of that, and the accessibility sweep
 * in `e2e/` queries these by role.
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
    router.path = strip(window.location.pathname);
  };

  document.addEventListener('click', onClick);
  window.addEventListener('popstate', onPop);

  return () => {
    document.removeEventListener('click', onClick);
    window.removeEventListener('popstate', onPop);
  };
}

/** Whether a navigation entry is the one being shown. */
export function isCurrent(path: string, exact = false): boolean {
  return exact ? router.path === path : router.path === path || router.path.startsWith(`${path}/`);
}
