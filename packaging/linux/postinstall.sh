#!/bin/sh
# dpkg calls this `configure <version replaced>`, with no version on a first install, and rpm
# with the count of copies installed, 1 on a first install. Debian and Ubuntu start a service
# their package installs. Fedora and RHEL leave it off until somebody turns it on, which the
# documentation says how to do. An upgrade restarts a running service on the new binary.
set -e

# No systemd running, as in a container or a chroot: the files are in place, nothing to start.
[ -d /run/systemd/system ] || exit 0
systemctl daemon-reload

case "$1" in
  configure)
    if [ -z "$2" ]; then
      systemctl enable --now routarr.service
    else
      systemctl try-restart routarr.service
    fi
    ;;
esac
