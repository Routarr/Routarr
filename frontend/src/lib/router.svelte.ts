import { basePath } from '../api/basePath';

/**
 * The whole router. Thirteen flat routes, no nesting, no parameters.
 *
 * Written rather than installed, for one reason that is specific to this
 * application: the mount point is discovered at *runtime* from the `<base href>`
 * the backend injects, because a single Docker image has to serve any sub-path.
 * Every SPA router worth taking wants its base at build time, which would mean
 * one image per mount point — the opposite of what this project ships.
 *
 * Fifty lines is also less than the cost of tracking a routing dependency's
 * position on Svelte 5.
 */

const strip = (pathname: string) => {
  const base = basePath();
  const path = base && pathname.startsWith(base) ? pathname.slice(base.length) : pathname;
  return path.startsWith('/') ? path : `/${path}`;
};

/** The current route, without the mount point. Reactive. */
export const router = $state({ path: strip(window.location.pathname) });

/**
 * The `href` of a route: the mount point and the path.
 *
 * A `<base href>` applies to relative URLs only, and these anchors are
 * root-absolute. Written as `/rules`, the left click worked because the
 * interception below prefixed the mount point — and everything else a native
 * link offers (middle-click, ctrl-click, the status bar, a copied address)
 * went to the proxy's root and a 404. Every anchor to a route goes through
 * this, so the attribute says where the click will actually go.
 */
export const href = (to: string) => `${basePath()}${to}`;

/** Follow a link the way the browser would, without reloading the document. */
export function navigate(to: string, options: { replace?: boolean } = {}) {
  const url = `${basePath()}${to}`;
  if (options.replace) window.history.replaceState({}, '', url);
  else window.history.pushState({}, '', url);
  router.path = strip(window.location.pathname);
  window.scrollTo(0, 0);
}

/**
 * One delegated listener rather than a `<Link>` component.
 *
 * The markup stays plain `<a href="/rules">`, which is what makes a link a link:
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

    event.preventDefault();
    navigate(strip(target));
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
