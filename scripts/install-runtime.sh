#!/usr/bin/env bash
# Install an isolating runtime on a CI runner and register it with Docker, so
# `scripts/smoke-image.sh` can start the image under it.
#
#   sudo bash scripts/install-runtime.sh gvisor|kata
#
# For an Ubuntu amd64 runner with Docker 26 or later, not for an installation:
# the README says how a host installs each one. Versions and checksums are
# pinned here, so an upstream release changes nothing until this file does.
set -euo pipefail

RUNTIME="${1:?usage: install-runtime.sh gvisor|kata}"

# gVisor, a kernel in user space between the container and the host's. The
# tarball holds `runsc` and the `gvisor-bin/` sidecars it looks for beside
# itself, so both land in one directory.
GVISOR_RELEASE=20260928
GVISOR_SHA512=4ce35ca83aef7f96b06cde668e0b23aa98b05aa1829508e974196c2a1e02786c95f5bf79315fd7ddcfd88fe7a00f083ed8053e25eff7673d28d5256440caae8b

# Kata Containers on its Rust runtime, one QEMU virtual machine per container:
# the only Kata path its maintainers test with Docker.
KATA_VERSION=4.2.0
KATA_SHA256=b828904fa3f1e49ddd7dc799c72cb1503cd1e772d354c3987c8d4189b2a623a8
KATA_SHIM=/opt/kata/runtime-rs/bin/containerd-shim-kata-v2
KATA_CONFIG=/opt/kata/share/defaults/kata-containers/runtime-rs/configuration-qemu-runtime-rs.toml

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
  kata)
    # QEMU runs the guest on the host's KVM. Without it the virtual machine
    # never boots, and the smoke test would report a timeout, not the cause.
    [ -c /dev/kvm ] || { echo "::error::this runner has no /dev/kvm, which Kata needs" >&2; exit 1; }
    curl -fsSL -o "$WORK/kata.tar.zst" \
      "https://github.com/kata-containers/kata-containers/releases/download/$KATA_VERSION/kata-static-$KATA_VERSION-amd64.tar.zst"
    echo "$KATA_SHA256  $WORK/kata.tar.zst" | sha256sum -c --quiet
    # The archive's paths start at /opt/kata.
    tar --zstd -xf "$WORK/kata.tar.zst" -C /
    register kata "$(jq -n --arg shim "$KATA_SHIM" --arg config "$KATA_CONFIG" \
      '{runtimeType: $shim, options: {ConfigPath: $config}}')"
    ;;
  *)
    echo "usage: install-runtime.sh gvisor|kata" >&2
    exit 2
    ;;
esac
