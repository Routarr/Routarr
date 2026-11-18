#!/usr/bin/env bash
# Sign the packages `scripts/build-packages.sh` built with a throwaway key, build the apt and dnf
# repositories from them as a release does, serve them on the loopback, and install from them:
# with apt on this machine, a CI runner, then with dnf in Fedora, each checking every signature.
#
#   bash scripts/smoke-repository.sh PACKAGES
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ $# -eq 1 ] || { sed -n '6p' "$0" >&2; exit 2; }
packages="$(cd "$1" && pwd)"

FEDORA="$(sed -n 's/^FEDORA=//p' "$ROOT/scripts/smoke-package.sh")"
URL=http://127.0.0.1:8081
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

work="$(mktemp -d)"
server=""
cleanup() {
  if [ -n "$server" ]; then kill "$server" 2>/dev/null || true; fi
  sudo rm -f /etc/apt/sources.list.d/routarr-smoke.list /usr/share/keyrings/routarr-smoke.gpg
  rm -rf "$work"
}
trap cleanup EXIT

# A key of this run alone, with a passphrase, as the release's.
export GNUPGHOME="$work/gnupg"
mkdir -m 0700 "$GNUPGHOME"
gpg --batch --quiet --pinentry-mode loopback --passphrase smoke \
  --quick-generate-key "Routarr packages smoke test" rsa3072 sign never
PACKAGES_GPG_KEY="$(gpg --batch --pinentry-mode loopback --passphrase smoke --armor \
  --export-secret-keys "Routarr packages smoke test")"
PACKAGES_GPG_PASSPHRASE=smoke
export PACKAGES_GPG_KEY PACKAGES_GPG_PASSPHRASE
gpgconf --kill gpg-agent
unset GNUPGHOME

mkdir "$work/release"
cp "$packages"/*.deb "$packages"/*.rpm "$work/release/"
bash "$ROOT/scripts/package-repository.sh" sign "$work/release"
(cd "$work/release" && sha256sum --quiet -c SHA256SUMS) || fail "SHA256SUMS does not match"
bash "$ROOT/scripts/package-repository.sh" build "$work/release" "$work/site" "$URL"
ok "signed and built the repositories"

python3 -m http.server 8081 --bind 127.0.0.1 --directory "$work/site" >/dev/null 2>&1 &
server=$!
for _ in $(seq 1 20); do curl -sf "$URL/routarr.asc" >/dev/null && break; sleep 0.5; done

debs=("$work"/release/*.deb)
version="$(dpkg-deb -f "${debs[0]}" Version)"
minor="$(cut -d. -f1,2 <<<"$version")"

gpg --dearmor <"$work/site/routarr.asc" | sudo tee /usr/share/keyrings/routarr-smoke.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/routarr-smoke.gpg] $URL/apt $minor main" |
  sudo tee /etc/apt/sources.list.d/routarr-smoke.list >/dev/null
sudo apt-get update -qq -o Dir::Etc::sourcelist=/etc/apt/sources.list.d/routarr-smoke.list \
  -o Dir::Etc::sourceparts=- -o APT::Get::List-Cleanup=0
sudo apt-get install -y -qq routarr >/dev/null
[ "$(dpkg-query -W -f '${Version}' routarr)" = "$version" ] || fail "apt installed another version"
ok "apt checks the $minor suite's signature and installs $version from it"
sudo apt-get purge -y -qq routarr >/dev/null

# The repository file a reader downloads, which asks dnf to check both signatures.
repo_file="$(cat "$work/site/rpm/routarr-stable.repo")"
for check in gpgcheck repo_gpgcheck; do
  grep -qx "$check=1" <<<"$repo_file" || fail "the repository file leaves $check off"
done
docker run --rm -i --network host "$FEDORA" bash -s <<INSIDE
set -euo pipefail
printf '%s\n' "$repo_file" >/etc/yum.repos.d/routarr.repo
dnf install -y -q routarr >/dev/null
rpm -q routarr
INSIDE
ok "dnf checks the stable repository's signature and the package's, and installs it"
echo "repository: every check passed"
