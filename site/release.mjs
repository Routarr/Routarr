/**
 * The version a visitor can pull: the latest release published on GitHub,
 * read when the site is built, by the page and by `check.mjs` alike.
 *
 * The site deploys from main, where `backend/Cargo.toml` is bumped before the
 * release it names is tagged, built and published: read from the crate alone,
 * the page names a version no image carries yet. The release page's redirect
 * names the tag without the API and its rate limit. Offline, or with GitHub not
 * answering, the crate's version stands in, so a build never fails on the
 * network.
 *
 * @param {string} crate the version `backend/Cargo.toml` declares
 * @returns {Promise<string>}
 */
export async function releasedVersion(crate) {
  try {
    const response = await fetch('https://github.com/Routarr/Routarr/releases/latest', {
      redirect: 'manual',
      signal: AbortSignal.timeout(10_000),
    });
    const location = response.headers.get('location') ?? '';
    return location.match(/\/releases\/tag\/v([^/?#]+)$/)?.[1] ?? crate;
  } catch {
    return crate;
  }
}
