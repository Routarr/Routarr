#!/usr/bin/env bash
# Build the tarball, the deb and the rpm of one architecture from the files of an image this
# repository built, so each ships the binary and the interface the image does.
#
#   bash scripts/build-packages.sh image IMAGE ARCH VERSION OUT
#   bash scripts/build-packages.sh folder DIR ARCH VERSION OUT
#
# `image` copies `/app` out of IMAGE, loaded in the local Docker. `folder` reads DIR laid out as
# `/app` is (`routarr`, `frontend/dist`, `LICENSE`), to try the packaging without Docker.
# ARCH is amd64 or arm64, VERSION the crate's without its `v`, OUT the folder written to.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ $# -eq 5 ] || { sed -n '5,6p' "$0" >&2; exit 2; }
source=$1 from=$2 arch=$3 version=$4 out=$5

case "$arch" in
  amd64) machine='3e00' ;;
  arm64) machine='b700' ;;
  *) echo "ARCH is amd64 or arm64, not $arch" >&2; exit 2 ;;
esac
command -v nfpm >/dev/null || { echo "nfpm is missing: sudo bash scripts/install-nfpm.sh" >&2; exit 1; }

stage="$(mktemp -d)"
container=""
cleanup() {
  rm -rf "$stage"
  if [ -n "$container" ]; then docker rm -f "$container" >/dev/null; fi
}
trap cleanup EXIT

case "$source" in
  image)
    container="$(docker create --platform "linux/$arch" "$from")"
    docker cp "$container:/app/routarr" "$stage/routarr"
    docker cp "$container:/app/frontend/dist" "$stage/frontend"
    docker cp "$container:/app/LICENSE" "$stage/LICENSE"
    ;;
  folder)
    cp "$from/routarr" "$stage/routarr"
    cp -R "$from/frontend/dist" "$stage/frontend"
    cp "$from/LICENSE" "$stage/LICENSE"
    ;;
  *) echo "the source is image or folder, not $source" >&2; exit 2 ;;
esac

# The ELF header's machine field, so a binary of the other architecture is never packaged.
found="$(od -An -tx1 -j18 -N2 "$stage/routarr" | tr -d ' \n')"
[ "$found" = "$machine" ] || { echo "routarr is no $arch binary (ELF machine $found)" >&2; exit 1; }

# The image's notices end with the packages of its Alpine base, which a package does not carry.
# musl takes their place: the binary links it statically.
notices="$stage/frontend/third-party-notices.txt"
marker='# Packages of the Alpine base'
if [ "$source" = image ]; then
  grep -qxF "$marker" "$notices" || { echo "the image's notices lost '$marker'" >&2; exit 1; }
fi
awk -v marker="$marker" '$0 == marker { exit } { print }' "$notices" >"$stage/notices"
{
  cat "$stage/notices"
  printf '# musl\n\nThe C library the binary links statically, from https://musl.libc.org/.\n\n'
  cat "$ROOT/LICENSES/MIT-musl.txt"
} >"$notices"
rm "$stage/notices"

cp "$ROOT"/packaging/linux/{nfpm.yaml,routarr.service,copyright} "$stage/"
cp "$ROOT"/packaging/linux/{preinstall,postinstall,preremove,postremove}.sh "$stage/"
for format in deb rpm; do
  sed "s/@INSTALL@/$format/" "$ROOT/packaging/linux/routarr.env" >"$stage/routarr-$format.env"
done

mkdir -p "$out"
out="$(cd "$out" && pwd)"
# The commit's time on every file, so one commit packages into the same bytes twice.
SOURCE_DATE_EPOCH="$(git -C "$ROOT" log -1 --format=%ct)"
export SOURCE_DATE_EPOCH ARCH="$arch" VERSION="$version"
find "$stage" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
for format in deb rpm; do
  (cd "$stage" && nfpm package --config nfpm.yaml --packager "$format" --target "$out/")
done

# Unpacked anywhere, the binary finds `frontend/dist` beside itself.
tarball="routarr-$version-linux-$arch"
mkdir -p "$stage/$tarball/frontend"
cp "$stage/routarr" "$stage/LICENSE" "$stage/$tarball/"
cp -R "$stage/frontend" "$stage/$tarball/frontend/dist"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
  -C "$stage" -czf "$out/$tarball.tar.gz" "$tarball"

ls -1 "$out"
