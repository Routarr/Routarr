#!/usr/bin/env bash
# Install gVisor on an Ubuntu amd64 CI runner and register it with Docker as `runsc`, so
# `scripts/smoke-image.sh` can start the image under it. A host follows the README instead.
#
#   sudo bash scripts/install-runtime.sh gvisor
set -euo pipefail

RUNTIME="${1:?usage: install-runtime.sh gvisor}"

GVISOR_RELEASE=20260928
GVISOR_SHA512=4ce35ca83aef7f96b06cde668e0b23aa98b05aa1829508e974196c2a1e02786c95f5bf79315fd7ddcfd88fe7a00f083ed8053e25eff7673d28d5256440caae8b

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Merges into the daemon's configuration, since a runner ships one of its own.
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
    # `runsc` looks for its `gvisor-bin/` sidecars beside itself, so both land here.
    tar --zstd -xf "$WORK/gvisor.tar.zstd" -C /usr/local/bin
    register runsc '{"path": "/usr/local/bin/runsc"}'
    ;;
  *)
    echo "usage: install-runtime.sh gvisor" >&2
    exit 2
    ;;
esac
