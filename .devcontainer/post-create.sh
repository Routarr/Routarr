#!/usr/bin/env bash
# Create step for the dev container. Each step skips its slow path when nothing
# changed, or every rebuild reads as a hung "Rebuild container".
set -euo pipefail

# The container's age from PID 1 (/proc/uptime is the host's): minutes here
# are VS Code installing its server, not this script.
started="$(stat -c %Y /proc/1 2>/dev/null || echo 0)"
age=$(( $(date +%s) - started ))
printf 'container %dm %02ds old at the create step\n' "$((age / 60))" "$((age % 60))"

root="$(cd "$(dirname "$0")/.." && pwd)"

# Both npm trees, the interface and the site, without the install scripts of their packages,
# which run with the credentials this container holds. The packages named after a tree run
# theirs: a new one that needs its script is added here in a reviewed change.
install_if_stale() {
  dir="$1"
  shift
  cd "$root/$dir"
  stamp="node_modules/.package-lock.sha256"
  want="$(sha256sum package-lock.json | cut -d' ' -f1)"
  if [ -d node_modules ] && [ "$(cat "$stamp" 2>/dev/null || true)" = "$want" ]; then
    echo "$dir/node_modules already matches package-lock.json, skipping npm ci"
  else
    npm ci --ignore-scripts
    if [ "$#" -gt 0 ]; then npm rebuild "$@"; fi
    printf '%s\n' "$want" > "$stamp"
  fi
}

install_if_stale frontend
install_if_stale site esbuild workerd

# Cargo looks in $CARGO_HOME/bin first, and an old `cargo install` left on the
# /cargo volume would shadow the image's pinned build.
for tool in cargo-llvm-cov cargo-deny cargo-about cargo-mutants; do
  if [ -x "/usr/local/bin/$tool" ] && [ -e "${CARGO_HOME:-/cargo}/bin/$tool" ]; then
    rm -f "${CARGO_HOME:-/cargo}/bin/$tool"
    echo "removed a stale $tool from ${CARGO_HOME:-/cargo}/bin: the image's pinned build is the one that runs"
  fi
done

# A no-op when the volume holds them: the three engines e2e runs in.
cd "$root/frontend"
npx playwright install chromium firefox webkit

