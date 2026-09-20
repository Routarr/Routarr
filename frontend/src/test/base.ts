/**
 * Install or remove the `<base href>` the backend injects, so a test can stand
 * where a reverse proxy puts the application. `basePath()` reads it on every
 * call, which is what lets a test change it after the module loaded.
 */
export function withBase(path: string | null) {
  document.head.querySelector('base')?.remove();
  if (path === null) return;
  const base = document.createElement('base');
  base.setAttribute('href', path);
  document.head.append(base);
}
