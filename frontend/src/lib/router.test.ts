import { describe, it, expect, beforeEach, vi, afterEach, onTestFinished } from 'vitest';
import { guardLeaving, href, interceptLinks, isCurrent, navigate, router } from './router.svelte';
import { withBase } from '../test/base';
import { click } from '../test/links';

/**
 * Flat routes, written rather than installed, because the mount point is
 * discovered at runtime from the `<base href>` the backend injects, and every
 * SPA router worth taking wants its base at build time.
 */

beforeEach(() => {
  withBase(null);
  window.history.replaceState({}, '', '/');
  router.path = '/';
});

afterEach(() => {
  withBase(null);
  vi.restoreAllMocks();
});

describe('navigation', () => {
  it('moves without reloading the document', () => {
    navigate('/rules');

    expect(router.path).toBe('/rules');
    expect(window.location.pathname).toBe('/rules');
  });

  /** What the browser does for a fragment link, and nothing when the page changes. */
  it('announces a new fragment of the page on screen, as the browser would', () => {
    const heard: string[] = [];
    const listen = (event: HashChangeEvent) => heard.push(new URL(event.newURL).hash);
    window.addEventListener('hashchange', listen);
    try {
      navigate('/settings#metadata');
      navigate('/settings#guardrails');
      navigate('/settings#guardrails');
    } finally {
      window.removeEventListener('hashchange', listen);
    }

    expect(heard).toEqual(['#guardrails']);
  });

  /** As a link does in the browser: Back would otherwise take two presses to leave. */
  it('adds no history entry for the page already on screen', () => {
    navigate('/rules');
    const push = vi.spyOn(window.history, 'pushState');

    navigate('/rules');

    expect(push).not.toHaveBeenCalled();
    expect(router.path).toBe('/rules');
  });

  /**
   * A sub-path is the case that only ever breaks behind a reverse proxy, where
   * it is hardest to diagnose. The router has to strip it, or every route
   * misses and the dashboard is shown for everything.
   */
  it('strips the mount point a reverse proxy adds', () => {
    withBase('/routarr/');
    navigate('/rules');

    expect(window.location.pathname).toBe('/routarr/rules');
    expect(router.path).toBe('/rules');
  });

  it('answers with a leading slash even at the mount point itself', () => {
    withBase('/routarr/');
    navigate('/');

    expect(router.path).toBe('/');
  });

  /** `/routarr` is a prefix of `/routarrX`, which is not under it. */
  it('does not take a path beside the mount point for one under it', () => {
    withBase('/routarr/');
    const stop = interceptLinks();
    try {
      window.history.replaceState({}, '', '/routarrX/rules');
      window.dispatchEvent(new PopStateEvent('popstate'));

      expect(router.path).toBe('/routarrX/rules');
    } finally {
      stop();
    }
  });
});

describe('the href of a route', () => {
  /**
   * A `<base href>` applies to relative URLs only, and every anchor here is
   * root-absolute. Without the mount point, every way of following a link (a
   * click, middle-click, ctrl-click, the status bar, copy link) reaches the
   * proxy's root and a 404.
   */
  it('carries the mount point, so every affordance of a link lands here', () => {
    withBase('/routarr/');
    expect(href('/rules')).toBe('/routarr/rules');
    expect(href('/')).toBe('/routarr/');
  });

  it('is the bare path when there is no mount point', () => {
    expect(href('/rules')).toBe('/rules');
  });
});

describe('the current entry', () => {
  it('leaves dark an entry whose path only starts the current one', () => {
    navigate('/rule-tests');

    expect(isCurrent('/rules')).toBe(false);
    expect(isCurrent('/rule-tests')).toBe(true);
    expect(isCurrent('/')).toBe(false);
  });

  it('treats a child path as being under its section', () => {
    navigate('/rules/7');

    expect(isCurrent('/rules')).toBe(true);
  });
});

describe('intercepting links', () => {
  /** An anchor in the document, taken out again when the test finishes. */
  function link(to: string, attributes: Record<string, string> = {}): HTMLAnchorElement {
    const anchor = document.createElement('a');
    anchor.setAttribute('href', to);
    for (const [name, value] of Object.entries(attributes)) anchor.setAttribute(name, value);
    document.body.append(anchor);
    onTestFinished(() => anchor.remove());
    return anchor;
  }

  /** The router listening for the length of the test. */
  function listening() {
    onTestFinished(interceptLinks());
  }

  /**
   * One delegated listener rather than a `<Link>` component: the markup stays a
   * plain `<a href>`, which is what makes middle-click open a tab and a screen
   * reader announce a link with a destination.
   */
  it('follows an internal link without leaving the document', () => {
    listening();

    expect(click(link('/instances'))).toBe(true);
    expect(router.path).toBe('/instances');
  });

  it('follows a prefixed link under a mount point without doubling the prefix', () => {
    withBase('/routarr/');
    listening();

    click(link(href('/instances')));

    expect(router.path).toBe('/instances');
    expect(window.location.pathname).toBe('/routarr/instances');
  });

  it('leaves a link to another application on the same host to the browser', () => {
    withBase('/routarr/');
    listening();

    expect(click(link('/radarr/'))).toBe(false);
  });

  /**
   * No screen lives under `/api`: a link there is the server's to answer. The
   * sign-in with an identity provider is one, and, taken by the router, it
   * changes the address and leaves the screen where it was.
   */
  it.each([
    ['without a mount point', null, '/api/v1/auth/oidc/start'],
    ['under a mount point', '/routarr/', '/routarr/api/v1/auth/oidc/start'],
  ])('leaves a link into the API to the browser, %s', (_, base, to) => {
    withBase(base);
    listening();

    expect(click(link(to))).toBe(false);
    expect(router.path).toBe('/');
  });

  /** `/api` is a path segment, not a prefix: `/apix` would be a screen's. */
  it('still follows a path that only begins with the letters api', () => {
    listening();

    expect(click(link('/apix'))).toBe(true);
    expect(router.path).toBe('/apix');
  });

  it('leaves an external link to the browser', () => {
    listening();

    expect(click(link('https://github.com/Routarr/Routarr'))).toBe(false);
    expect(router.path).toBe('/');
  });

  it('leaves a modified click alone, so ctrl-click still opens a tab', () => {
    listening();

    expect(click(link('/instances'), { ctrlKey: true })).toBe(false);
    expect(router.path).toBe('/');
  });

  it('leaves a download alone', () => {
    listening();

    expect(click(link('/backups/x.zip', { download: 'x.zip' }))).toBe(false);
  });

  it('follows the back button', () => {
    listening();
    navigate('/rules');

    window.history.replaceState({}, '', '/move-log');
    window.dispatchEvent(new PopStateEvent('popstate'));

    expect(router.path).toBe('/move-log');
  });

  it('stops listening once it is told to', () => {
    const stop = interceptLinks();
    stop();

    expect(click(link('/instances'))).toBe(false);
    expect(router.path).toBe('/');
  });
});

/**
 * A screen holding unsaved work is asked before it goes: by a link, by a
 * screen's own navigation, by Back. A move within the screen asks nothing.
 */
describe('a screen that guards its work', () => {
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

  function guarded(answer: boolean) {
    const asked = vi.fn((to: string) => {
      void to;
      return Promise.resolve(answer);
    });
    onTestFinished(guardLeaving(asked));
    return asked;
  }

  beforeEach(() => navigate('/settings'));

  it('stays when the screen keeps its work', async () => {
    const asked = guarded(false);

    navigate('/rules');
    await settle();

    expect(asked).toHaveBeenCalledWith('/rules');
    expect(router.path).toBe('/settings');
  });

  it('goes once the screen lets it', async () => {
    guarded(true);

    navigate('/rules');
    await settle();

    expect(router.path).toBe('/rules');
  });

  it('asks nothing for another section of the same screen', () => {
    const asked = guarded(false);

    navigate('/settings#guardrails');

    expect(asked).not.toHaveBeenCalled();
    expect(window.location.hash).toBe('#guardrails');
  });

  it('puts the address back when Back is refused', async () => {
    onTestFinished(interceptLinks());
    guarded(false);

    window.history.pushState({}, '', '/rules');
    window.dispatchEvent(new PopStateEvent('popstate'));
    await settle();

    expect(router.path).toBe('/settings');
    expect(window.location.pathname).toBe('/settings');
  });

  it('asks nothing once the screen has gone', async () => {
    const asked = vi.fn(() => Promise.resolve(false));
    guardLeaving(asked)();

    navigate('/rules');
    await settle();

    expect(asked).not.toHaveBeenCalled();
    expect(router.path).toBe('/rules');
  });
});
