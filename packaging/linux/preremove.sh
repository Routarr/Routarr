#!/bin/sh
# dpkg calls this `remove` before a removal and `upgrade` before an upgrade, and rpm with the
# count of copies left, 0 for a removal. Only a removal stops and disables the service: an
# upgrade restarts it once the new files are in place.
set -e

[ -d /run/systemd/system ] || exit 0

case "$1" in
  remove | 0)
    systemctl disable --now routarr.service || true
    ;;
esac
