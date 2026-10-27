#!/usr/bin/env bash
# A Playwright browser and the system packages it runs on, for a CI job, from what its cache
# restored.
#
#   bash scripts/install-browser.sh key BROWSER       the cache key and its prefix, for $GITHUB_OUTPUT
#   bash scripts/install-browser.sh install BROWSER   the browser and its packages
#
# The Ubuntu mirror can crawl for many minutes over WebKit's packages, so the archives kept in
# ~/.cache/playwright-debs are offered to apt first. apt takes one only when its sum matches the
# signed index and downloads whatever changed since, so a stale or altered file is never
# installed. The browser is kept in ~/.cache/ms-playwright, where Playwright deletes the
# revisions its version does not name.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCHIVES=/var/cache/apt/archives
KEPT="$HOME/.cache/playwright-debs"
mode="${1:-}"
browser="${2:?install-browser: name a browser, chromium, firefox or webkit}"

case "$mode" in
  key)
    version=$(node -p 'require(process.argv[1]).version' "$ROOT/frontend/node_modules/playwright-core/package.json")
    # An archive is built for one Ubuntu release, which the runner names.
    prefix="playwright-${ImageOS:?install-browser: the runner names no ImageOS}-$browser-"
    echo "prefix=$prefix"
    echo "key=$prefix$version"
    ;;
  install)
    mkdir -p "$KEPT"
    shopt -s nullglob
    kept=("$KEPT"/*.deb)
    [ "${#kept[@]}" -eq 0 ] || sudo cp "${kept[@]}" "$ARCHIVES/"
    # apt-get's default, stated so that the archives this run downloads are there to keep.
    echo 'APT::Keep-Downloaded-Packages "true";' | sudo tee /etc/apt/apt.conf.d/99keep-archives >/dev/null
    (cd "$ROOT/frontend" && npx playwright install --with-deps "$browser")
    # An archive the index does not list goes, so those kept follow the mirror rather than grow.
    sudo apt-get autoclean -qq
    rm -f "$KEPT"/*.deb
    fetched=("$ARCHIVES"/*.deb)
    [ "${#fetched[@]}" -eq 0 ] || cp "${fetched[@]}" "$KEPT/"
    echo "install-browser: $browser installed, ${#kept[@]} archive(s) offered from the cache, ${#fetched[@]} kept"
    ;;
  *)
    echo "install-browser: the mode is key or install, not '$mode'" >&2
    exit 2
    ;;
esac
