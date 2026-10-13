/**
 * The sub-path the application is mounted under, without a trailing slash.
 *
 * It is a server setting, read at run time from the `<base href>` the backend injects. The dev
 * server injects none, and `document.baseURI` is then the page URL, wrong on a deep link.
 */
export function basePath(): string {
  if (typeof document === 'undefined') return '';

  const hasBaseTag = document.querySelector('base[href]') !== null;
  if (!hasBaseTag) return '';

  try {
    return new URL(document.baseURI).pathname.replace(/\/$/, '');
  } catch {
    return '';
  }
}
