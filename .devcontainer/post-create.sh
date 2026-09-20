#!/usr/bin/env bash
# Create step for the dev container. Both halves are idempotent and skip their
# expensive path when nothing changed. A plain
# `npm ci && npx playwright install --with-deps chromium` would, on every
# rebuild, wipe 200 MB of node_modules across the bind mount and run apt-get
# again — several silent minutes that read as a hung "Rebuild container".
# The system libraries live in the image; only the browser, persisted in the
# /playwright volume, can still be missing here.
set -euo pipefail

# Age of this container, taken from PID 1: there is no time namespace here, so
# /proc/uptime reports the host's. Everything before this point is VS Code
# connecting and installing its server, which it reports nowhere. Seconds is a
# healthy start; minutes is the editor, not this container.
started="$(stat -c %Y /proc/1 2>/dev/null || echo 0)"
age=$(( $(date +%s) - started ))
printf 'container %dm %02ds old at the create step\n' "$((age / 60))" "$((age % 60))"

root="$(cd "$(dirname "$0")/.." && pwd)"

# Two npm trees: the interface and the showcase site. Preparing only
# `frontend/` leaves a fresh container unable to run `npm run build` or
# `node site/check.mjs`, which fails in a way that reads as a broken checkout
# rather than a missing install.
install_if_stale() {
  dir="$1"
  cd "$root/$dir"
  stamp="node_modules/.package-lock.sha256"
  want="$(sha256sum package-lock.json | cut -d' ' -f1)"
  if [ -d node_modules ] && [ "$(cat "$stamp" 2>/dev/null || true)" = "$want" ]; then
    echo "$dir/node_modules already matches package-lock.json — skipping npm ci"
  else
    npm ci
    printf '%s\n' "$want" > "$stamp"
  fi
}

install_if_stale frontend
install_if_stale site

# The image ships cargo-audit and cargo-llvm-cov, pinned by checksum, in
# /usr/local/bin. Cargo resolves a `cargo-<name>` subcommand from $CARGO_HOME/bin
# first, and /cargo is a volume that outlives the image: a copy left there by an
# old `cargo install` shadows the pinned one silently. The volume keeps its
# downloads; it does not get to keep a tool the image provides.
for tool in cargo-audit cargo-llvm-cov; do
  if [ -x "/usr/local/bin/$tool" ] && [ -e "${CARGO_HOME:-/cargo}/bin/$tool" ]; then
    rm -f "${CARGO_HOME:-/cargo}/bin/$tool"
    echo "removed a stale $tool from ${CARGO_HOME:-/cargo}/bin — the image's pinned build is the one that runs"
  fi
done

# Fast no-op when the volume already holds this Chromium build.
cd "$root/frontend"
npx playwright install chromium

# Register RTK's hook with Claude Code. Idempotent, and cheap enough to rerun:
# it writes into ~/.claude, and a fresh volume starts without it.
#
# The hook rewrites Bash commands (`git status` becomes `rtk git status`) so the
# agent reads compressed output. It only covers the Bash tool: Claude Code's own
# Read, Grep and Glob do not pass through it.
if command -v rtk > /dev/null; then
  rtk init -g && echo "RTK hook registered — restart Claude Code for it to take effect"
else
  echo "RTK is not installed; rebuild the container to get it" >&2
fi
