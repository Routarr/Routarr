/**
 * The one inline script: it has to run before first paint to avoid a flash of
 * the wrong theme, so it cannot be bundled and deferred. Its `sha256-` is
 * pinned in `public/_headers` and recomputed by `check.mjs`, which is why the
 * string lives in one place — a second copy that drifts by a character is a
 * script the CSP blocks on one page and not the other.
 */
export const themeBootstrap =
  '(function(){try{var t=localStorage.getItem("routarr.theme");if(t){document.documentElement.dataset.theme=t}}catch(e){}})();';
