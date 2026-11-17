#!/bin/sh
# The service's account, made before the files it owns are unpacked. A system account with no
# login, kept when the package goes: the library it owns may outlive the package.
set -e

if ! getent group routarr >/dev/null; then
  groupadd --system routarr
fi
if ! getent passwd routarr >/dev/null; then
  useradd --system --gid routarr --home-dir /var/lib/routarr --no-create-home \
    --shell /usr/sbin/nologin --comment "Routarr" routarr
fi
