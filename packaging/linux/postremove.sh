#!/bin/sh
# dpkg calls this `remove`, `purge` or `upgrade`, and rpm with the count of copies left, 1 after
# an upgrade. A removal keeps the library, its keys and its backups in /var/lib/routarr. A purge,
# which only dpkg has, deletes them with the settings, as it is asked to. The account stays,
# since a file outside the package may still be its.
set -e

if [ -d /run/systemd/system ]; then
  systemctl daemon-reload || true
fi

case "$1" in
  purge)
    rm -rf /var/lib/routarr /etc/routarr
    ;;
  1)
    # rpm restarts after an upgrade here, once the old package's files are gone.
    if [ -d /run/systemd/system ]; then
      systemctl try-restart routarr.service || true
    fi
    ;;
esac
