#!/usr/bin/env bash
# Sign the packages a release ships, and build the apt and dnf repositories that serve them.
#
#   bash scripts/package-repository.sh sign DIR
#   bash scripts/package-repository.sh build PACKAGES OUT [URL]
#
# `sign` signs each rpm of DIR, then writes `SHA256SUMS` over every file there and its detached
# signature `SHA256SUMS.asc`. `build` writes into OUT the repositories of the deb and rpm files of
# PACKAGES, one suite per minor line and `stable` for the newest, as the image's `X.Y` and
# `latest` tags, served from URL (https://packages.routarr.app by default).
#
# The key is the armoured secret key in PACKAGES_GPG_KEY, with its passphrase, if any, in
# PACKAGES_GPG_PASSPHRASE. It is imported into a keyring of its own, deleted on exit.
set -euo pipefail

[ $# -ge 2 ] || { sed -n '4,5p' "$0" >&2; exit 2; }
[ -n "${PACKAGES_GPG_KEY:-}" ] || { echo "PACKAGES_GPG_KEY holds no key" >&2; exit 1; }

GNUPGHOME="$(mktemp -d)"
export GNUPGHOME
trap 'gpgconf --kill gpg-agent 2>/dev/null || true; rm -rf "$GNUPGHOME"' EXIT
passphrase="$GNUPGHOME/passphrase"
printf '%s' "${PACKAGES_GPG_PASSPHRASE:-}" >"$passphrase"
gpg --batch --quiet --pinentry-mode loopback --passphrase-file "$passphrase" \
  --import <<<"$PACKAGES_GPG_KEY"
fingerprint="$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "fpr" { print $10; exit }')"
[ -n "$fingerprint" ] || { echo "PACKAGES_GPG_KEY holds no secret key" >&2; exit 1; }
signing=(gpg --batch --yes --pinentry-mode loopback --passphrase-file "$passphrase" -u "$fingerprint")

sign() {
  local dir=$1 file
  local rpmdb
  rpmdb="$(mktemp -d)"
  gpg --batch --armor --export "$fingerprint" >"$rpmdb/key.asc"
  rpm --dbpath "$rpmdb" --import "$rpmdb/key.asc"
  for file in "$dir"/*.rpm; do
    rpmsign --addsign \
      --define "_gpg_name $fingerprint" \
      --define "_gpg_sign_cmd_extra_args --batch --pinentry-mode loopback --passphrase-file $passphrase" \
      "$file" >/dev/null
    rpm --dbpath "$rpmdb" --checksig "$file" | grep -q "signatures OK" ||
      { echo "$file carries no signature the key checks" >&2; exit 1; }
  done
  rm -rf "$rpmdb"
  (cd "$dir" && find . -maxdepth 1 -type f ! -name 'SHA256SUMS*' -printf '%f\n' | sort |
    xargs sha256sum >SHA256SUMS)
  "${signing[@]}" --armor --detach-sign --output "$dir/SHA256SUMS.asc" "$dir/SHA256SUMS"
  echo "signed $(find "$dir" -name '*.rpm' | wc -l) rpm and the sums of $(wc -l <"$dir/SHA256SUMS") files"
}

# The minor line `0.3` of a file named by its version 0.3.1, deb or rpm alike.
minor_of() { sed -E 's/^routarr[-_]([0-9]+\.[0-9]+)\..*/\1/' <<<"$1"; }

build() {
  local packages=$1 out=$2 url=${3:-https://packages.routarr.app}
  local file minor suite
  rm -rf "$out"
  mkdir -p "$out/apt" "$out/rpm/packages"

  local minors=()
  for file in "$packages"/*.deb "$packages"/*.rpm; do
    [ -e "$file" ] || continue
    case "$(basename "$file")" in
      *~*) echo "a pre-release has no place in a repository: $file" >&2; exit 1 ;;
    esac
    minors+=("$(minor_of "$(basename "$file")")")
  done
  [ ${#minors[@]} -gt 0 ] || { echo "no package in $packages" >&2; exit 1; }
  mapfile -t minors < <(printf '%s\n' "${minors[@]}" | sort -u -V)
  local newest=${minors[-1]}

  for file in "$packages"/*.deb; do
    minor="$(minor_of "$(basename "$file")")"
    mkdir -p "$out/apt/pool/$minor"
    cp "$file" "$out/apt/pool/$minor/"
  done
  cp "$packages"/*.rpm "$out/rpm/packages/"

  for suite in "${minors[@]}" stable; do
    minor=$suite
    [ "$suite" = stable ] && minor=$newest

    local dist="$out/apt/dists/$suite"
    for arch in amd64 arm64; do
      mkdir -p "$dist/main/binary-$arch"
      (cd "$out/apt" && apt-ftparchive --arch "$arch" packages "pool/$minor") \
        >"$dist/main/binary-$arch/Packages"
      gzip -9 -n -k "$dist/main/binary-$arch/Packages"
    done
    (cd "$out/apt" && apt-ftparchive \
      -o APT::FTPArchive::Release::Origin=Routarr \
      -o APT::FTPArchive::Release::Label=Routarr \
      -o APT::FTPArchive::Release::Suite="$suite" \
      -o APT::FTPArchive::Release::Codename="$suite" \
      -o APT::FTPArchive::Release::Architectures="amd64 arm64" \
      -o APT::FTPArchive::Release::Components=main \
      release "dists/$suite") >"$GNUPGHOME/Release"
    mv "$GNUPGHOME/Release" "$dist/Release"
    "${signing[@]}" --clearsign --output "$dist/InRelease" "$dist/Release"
    "${signing[@]}" --armor --detach-sign --output "$dist/Release.gpg" "$dist/Release"

    local listed
    listed="$(mktemp -d)"
    cp "$out"/rpm/packages/routarr-"$minor".*.rpm "$listed/"
    mkdir -p "$out/rpm/$suite"
    # gz, which every dnf reads, where a newer createrepo_c defaults to zstd.
    createrepo_c --quiet --general-compress-type gz --baseurl "$url/rpm/packages/" \
      --outputdir "$out/rpm/$suite" "$listed"
    rm -rf "$listed"
    "${signing[@]}" --armor --detach-sign --output "$out/rpm/$suite/repodata/repomd.xml.asc" \
      "$out/rpm/$suite/repodata/repomd.xml"
    cat >"$out/rpm/routarr-$suite.repo" <<REPO
[routarr]
name=Routarr $suite
baseurl=$url/rpm/$suite
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=$url/routarr.asc
REPO
  done

  gpg --batch --armor --export "$fingerprint" >"$out/routarr.asc"
  cat >"$out/index.html" <<PAGE
<!doctype html>
<meta charset="utf-8">
<title>Routarr packages</title>
<p>The apt and dnf repositories of <a href="https://routarr.app">Routarr</a>, signed with the key
<a href="routarr.asc">routarr.asc</a>, $fingerprint.
<a href="https://github.com/Routarr/Routarr/blob/main/OPERATING.md#without-docker">OPERATING.md</a>
says how to install from them.</p>
PAGE

  # GitHub Pages serves a site of 1 GB at most.
  local size
  size="$(du -sm "$out" | cut -f1)"
  [ "$size" -lt 900 ] || { echo "the repositories weigh $size MB, past what Pages serves" >&2; exit 1; }
  echo "suites ${minors[*]} stable, stable on $newest, $size MB, key $fingerprint"
}

case "$1" in
  sign) sign "$2" ;;
  build) build "$2" "$3" "${4:-}" ;;
  *) sed -n '4,5p' "$0" >&2; exit 2 ;;
esac
