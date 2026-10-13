/**
 * Installs the `<base href>` the backend injects, or removes it with `null`. `basePath()`
 * reads it on every call, so a test can change it after the module loaded.
 */
export function withBase(path: string | null) {
  document.head.querySelector('base')?.remove();
  if (path === null) return;
  const base = document.createElement('base');
  base.setAttribute('href', path);
  document.head.append(base);
}
