#!/usr/bin/env bash
# Install nFPM, which builds the deb and the rpm (`packaging/linux/nfpm.yaml`), on an amd64 or
# arm64 Ubuntu runner, held to a release and to its archive's sum like every other tool CI
# downloads.
#
#   sudo bash scripts/install-nfpm.sh
set -euo pipefail

NFPM_VERSION=2.48.0
NFPM_SHA256_X64=6d7f9a28b2f397934403950fcbba83f598b08d02edbe79a0815b3b4897341750
NFPM_SHA256_ARM64=a9a75213763783b7669719e505474c748c5ad7dc66591a195619deb50f85c2f4

case "$(uname -m)" in
  x86_64) arch=x86_64 sum=$NFPM_SHA256_X64 ;;
  aarch64) arch=arm64 sum=$NFPM_SHA256_ARM64 ;;
  *)
    echo "nFPM is installed here on x86_64 and aarch64 alone, not $(uname -m)" >&2
    exit 1
    ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

curl -fsSL -o "$WORK/nfpm.tar.gz" \
  "https://github.com/goreleaser/nfpm/releases/download/v$NFPM_VERSION/nfpm_${NFPM_VERSION}_Linux_$arch.tar.gz"
echo "$sum  $WORK/nfpm.tar.gz" | sha256sum -c --quiet
tar -xzf "$WORK/nfpm.tar.gz" -C "$WORK" nfpm
install -m 0755 "$WORK/nfpm" /usr/local/bin/nfpm
nfpm --version
