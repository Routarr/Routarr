#!/usr/bin/env bash
# Install gVisor on a CI runner and register it with Docker as `runsc`, so
# `scripts/smoke-image.sh` can start the image under it.
#
#   sudo bash scripts/install-runtime.sh gvisor
#
# For an Ubuntu amd64 runner with Docker, not for an installation: the README
# says how a host installs it. The version and checksum are pinned here, so an
# upstream release changes nothing until this file does.
set -euo pipefail

RUNTIME="${1:?usage: install-runtime.sh gvisor}"

# gVisor, a kernel in user space between the container and the host's. The
# tarball holds `runsc` and the `gvisor-bin/` sidecars it looks for beside
# itself, so both land in one directory.
GVISOR_RELEASE=20260928
GVISOR_SHA512=4ce35ca83aef7f96b06cde668e0b23aa98b05aa1829508e974196c2a1e02786c95f5bf79315fd7ddcfd88fe7a00f083ed8053e25eff7673d28d5256440caae8b

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Merged into the daemon's configuration rather than written over it: a runner
# ships one of its own.
register() {
  local name=$1 runtime=$2 config=/etc/docker/daemon.json
  [ -s "$config" ] || echo '{}' >"$config"
  jq --arg name "$name" --argjson runtime "$runtime" '.runtimes[$name] = $runtime' \
    "$config" >"$WORK/daemon.json"
  install -m 0644 "$WORK/daemon.json" "$config"
  systemctl restart docker
  docker info --format '{{json .Runtimes}}' | jq -e --arg name "$name" 'has($name)' >/dev/null ||
    { echo "::error::Docker does not list the $name runtime after a restart" >&2; exit 1; }
  echo "Docker runs containers under $name on request."
}

case "$RUNTIME" in
  gvisor)
    curl -fsSL -o "$WORK/gvisor.tar.zstd" \
      "https://storage.googleapis.com/gvisor/releases/release/$GVISOR_RELEASE/x86_64/gvisor.tar.zstd"
    echo "$GVISOR_SHA512  $WORK/gvisor.tar.zstd" | sha512sum -c --quiet
    tar --zstd -xf "$WORK/gvisor.tar.zstd" -C /usr/local/bin
    register runsc '{"path": "/usr/local/bin/runsc"}'
    ;;
  *)
    echo "usage: install-runtime.sh gvisor" >&2
    exit 2
    ;;
esac
