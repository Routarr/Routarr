/**
 * Where the application is mounted.
 *
 * Routarr can be served under a sub-path (`https://host/routarr`), the Servarr
 * "URL base" convention. The mount point is a server-side setting, not a build
 * one — a single Docker image has to work under any of them — so the frontend
 * discovers it at runtime instead of having it compiled in.
 *
 * `document.baseURI` reflects the `<base href>` the backend injects into
 * `index.html`. Without a base tag it is just the document URL, which for a deep
 * link like `/rules` would give the wrong answer; the backend always injects
 * one, and the fallback below keeps the dev server (which does not) working.
 */
export function basePath(): string {
  if (typeof document === 'undefined') return '';

  const hasBaseTag = document.querySelector('base[href]') !== null;
  if (!hasBaseTag) return '';

  try {
    // Trailing slash removed so callers can concatenate `/api/v1` directly.
    return new URL(document.baseURI).pathname.replace(/\/$/, '');
  } catch {
    return '';
  }
}
