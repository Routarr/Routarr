#!/usr/bin/env bash
# Install a package `scripts/build-packages.sh` built and check what OPERATING.md promises of
# it, as `smoke-image.sh` does for the image.
#
#   bash scripts/smoke-package.sh deb routarr_X_amd64.deb   # an Ubuntu runner, with systemd
#   bash scripts/smoke-package.sh rpm routarr-X.x86_64.rpm  # Fedora, in Docker, without systemd
#   bash scripts/smoke-package.sh tarball routarr-X-linux-amd64.tar.gz
#
# The deb is installed for real on the machine it runs on, then purged: a CI runner, never a
# machine of yours.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ $# -eq 2 ] || { sed -n '5,7p' "$0" >&2; exit 2; }
format=$1 file="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"

# Fedora's current release, as Dockerfile pins its images: the tag for a reader, the digest for
# Docker.
FEDORA=fedora:45@sha256:a7d72ba2745b100a3351087472e5ee7b2450af7cf331e7673007e3817612839c

BASE=http://127.0.0.1:9876
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

# The command the first-run screen shows for a package, read from the screen itself, so the
# instruction a new user follows is the one this runs.
read_key=$(sed -n "s/^ *const PACKAGE_KEY_COMMAND = '\(.*\)';$/\1/p" \
  "$ROOT/frontend/src/components/ApiKeyGate.svelte")
[ -n "$read_key" ] || fail "PACKAGE_KEY_COMMAND not found in ApiKeyGate.svelte"

pings() { curl -sf --max-time 5 "$BASE/api/v1/ping" >/dev/null; }
wait_for_ping() {
  for _ in $(seq 1 60); do pings && return 0; sleep 1; done
  return 1
}
install_said() {
  curl -s --max-time 10 "$BASE/api/v1/auth/mode" | sed -n 's/.*"install":"\([a-z]*\)".*/\1/p'
}
check_key() {
  local key=$1
  [[ "$key" =~ ^[0-9a-f]{64}$ ]] || fail "\`$read_key\` printed something other than a key"
  local status
  status=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $key" "$BASE/api/v1/status")
  [ "$status" = 200 ] || fail "the key read with \`$read_key\` was refused: HTTP $status"
  ok "\`$read_key\` prints the key the server accepts"
}

smoke_deb() {
  sudo apt-get install -y -qq "$file" >/dev/null
  ok "installs with apt"
  systemctl is-enabled --quiet routarr || fail "the service is not enabled after a first install"
  systemctl is-active --quiet routarr || fail "the service is not running after a first install"
  wait_for_ping || { sudo journalctl -u routarr --no-pager | tail -40; fail "no answer on $BASE"; }
  ok "the service starts on install and answers"

  local pid uid
  pid=$(systemctl show -p MainPID --value routarr)
  uid=$(awk '/^Uid:/ { print $2 }' "/proc/$pid/status")
  [ "$uid" = "$(id -u routarr)" ] || fail "the service runs as uid $uid, not routarr's"
  ok "runs as routarr"
  [ "$(install_said)" = deb ] || fail "the service says it was installed as '$(install_said)'"
  sudo journalctl -u routarr --no-pager | grep -q "Serving frontend assets from: /usr/share/routarr/frontend" ||
    fail "the service does not serve /usr/share/routarr/frontend"
  ok "serves the interface the package installed, and says it is a deb"

  # Split into words on purpose: it is a command line with no quoting in it.
  # shellcheck disable=SC2086
  check_key "$($read_key)"

  # Run by hand from anywhere, a command reads the service's settings and its library.
  local out code=0
  out=$(cd / && sudo -u routarr routarr reset-account --force 2>&1) || code=$?
  [ "$code" = 0 ] || fail "\`sudo -u routarr routarr reset-account --force\` failed: $out"
  grep -q "/var/lib/routarr/routarr.password" <<<"$out" || fail "the command did not reach the service's library: $out"
  ok "a command run as routarr reaches the service's library"
  code=0
  out=$(cd / && sudo routarr reset-account --force 2>&1) || code=$?
  [ "$code" = 78 ] || fail "run as root, reset-account ended with $code, not 78: $out"
  grep -q "sudo -u routarr" <<<"$out" || fail "the refusal does not name sudo -u routarr: $out"
  ok "run as root, a command is refused and told to use sudo -u routarr"
  code=0
  out=$(cd / && sudo -u routarr timeout 20 routarr 2>&1) || code=$?
  grep -q "Another Routarr is running" <<<"$out" || fail "a second server was not refused ($code): $out"
  ok "a second server on the library is refused"

  local exposure
  exposure=$(systemd-analyze security routarr.service --no-pager | tail -1)
  echo "$exposure"
  grep -qE "(OK|SAFE)" <<<"$exposure" || fail "the unit's exposure is not OK: $exposure"
  ok "the unit's hardening rates OK or better"

  local key_before
  key_before=$(sudo cat /var/lib/routarr/routarr.api_key)
  sudo apt-get install -y -qq --reinstall "$file" >/dev/null
  wait_for_ping || fail "no answer after a reinstall"
  [ "$(sudo cat /var/lib/routarr/routarr.api_key)" = "$key_before" ] || fail "a reinstall changed the key"
  ok "an upgrade restarts the service and keeps its library"

  sudo systemctl stop routarr
  sudo journalctl -u routarr --no-pager | grep -q "Routarr stopped cleanly" || fail "the stop was not clean"
  ok "stops cleanly"

  sudo apt-get remove -y -qq routarr >/dev/null
  ! systemctl is-enabled --quiet routarr 2>/dev/null || fail "the service is still enabled after a removal"
  sudo test -f /var/lib/routarr/routarr.db || fail "a removal deleted the library"
  ok "a removal keeps the library"
  sudo apt-get purge -y -qq routarr >/dev/null
  ! sudo test -e /var/lib/routarr || fail "a purge left /var/lib/routarr"
  ! sudo test -e /etc/routarr || fail "a purge left /etc/routarr"
  ok "a purge deletes the library and the settings"
}

smoke_rpm() {
  docker run --rm -i -v "$file:/tmp/routarr.rpm:ro" -e READ_KEY="$read_key" "$FEDORA" bash -s <<'INSIDE'
set -euo pipefail
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }
dnf install -y -q --allowerasing /tmp/routarr.rpm sudo procps-ng curl util-linux >/dev/null
ok "installs with dnf on $(. /etc/os-release && echo "$PRETTY_NAME")"
[ "$(stat -c '%U:%G %a' /etc/routarr/routarr.env)" = "root:routarr 640" ] || fail "routarr.env: $(stat -c '%U:%G %a' /etc/routarr/routarr.env)"
[ "$(stat -c '%U:%G %a' /var/lib/routarr)" = "routarr:routarr 750" ] || fail "/var/lib/routarr: $(stat -c '%U:%G %a' /var/lib/routarr)"
test -f /usr/lib/systemd/system/routarr.service || fail "no unit installed"
test -s /usr/share/licenses/routarr/LICENSE || fail "no licence in /usr/share/licenses/routarr"
grep -q "^# musl" /usr/share/licenses/routarr/third-party-notices.txt || fail "the notices do not name musl"
ok "the account, the files and their owners are in place"

# setpriv switches to the account with no PAM session, which a container without systemd may
# not open.
(cd / && setpriv --reuid=routarr --regid=routarr --init-groups -- /usr/bin/routarr \
  >/tmp/routarr.log 2>&1) &
for _ in $(seq 1 60); do curl -sf --max-time 5 http://127.0.0.1:9876/api/v1/ping >/dev/null && break; sleep 1; done
curl -sf --max-time 5 http://127.0.0.1:9876/api/v1/ping >/dev/null || {
  ps -eo pid,user,stat,args; cat /tmp/routarr.log; fail "no answer"
}
grep -q '"install":"rpm"' <(curl -s http://127.0.0.1:9876/api/v1/auth/mode) || fail "it does not say it is an rpm"
grep -q "Data directory: /var/lib/routarr" /tmp/routarr.log || fail "it did not read /etc/routarr/routarr.env"
ok "started as routarr, it reads its settings and answers as an rpm"
# shellcheck disable=SC2086
key=$($READ_KEY)
[[ "$key" =~ ^[0-9a-f]{64}$ ]] || fail "\`$READ_KEY\` printed something other than a key"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $key" http://127.0.0.1:9876/api/v1/status)" = 200 ] || fail "the key was refused"
ok "\`$READ_KEY\` prints the key the server accepts"
pkill -TERM -x routarr
for _ in $(seq 1 30); do pgrep -x routarr >/dev/null || break; sleep 1; done
grep -q "Routarr stopped cleanly" /tmp/routarr.log || fail "the stop was not clean"
dnf remove -y -q routarr >/dev/null
test -f /var/lib/routarr/routarr.db || fail "a removal deleted the library"
ok "stops cleanly, and a removal keeps the library"
INSIDE
}

smoke_tarball() {
  local work
  work="$(mktemp -d)"
  tar -xzf "$file" -C "$work"
  local dir
  dir="$(find "$work" -mindepth 1 -maxdepth 1 -type d)"
  test -s "$dir/LICENSE" || fail "the tarball carries no LICENSE"
  # `env` replaces itself with the server, so `$!` is the process the stop is sent to.
  env -C "$work" ROUTARR_DB_PATH="$work/data/routarr.db" "$dir/routarr" >"$work/log" 2>&1 &
  local pid=$!
  wait_for_ping || { cat "$work/log"; fail "no answer on $BASE"; }
  grep -q "Serving frontend assets from: $dir/frontend/dist" "$work/log" ||
    fail "the binary does not find the interface beside itself"
  [ -z "$(install_said)" ] || fail "an unpacked binary says it was installed as '$(install_said)'"
  ok "unpacked anywhere, it serves the interface beside it and names no installation"
  kill -TERM "$pid"
  wait "$pid" || fail "the stop ended with $?"
  grep -q "Routarr stopped cleanly" "$work/log" || fail "the stop was not clean"
  ok "stops cleanly"
  rm -rf "$work"
}

case "$format" in
  deb) smoke_deb ;;
  rpm) smoke_rpm ;;
  tarball) smoke_tarball ;;
  *) sed -n '5,7p' "$0" >&2; exit 2 ;;
esac
echo "$format: every check passed"
