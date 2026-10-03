#!/usr/bin/env bash
# Prepare the tmpfs ramdisk, and say so loudly if it is not one.
#
# The mount is declared in `devcontainer.json`'s `runArgs` and made by the
# container runtime, and this only prepares what lives inside it. Runs on create
# and on every start, since a tmpfs is empty again after each, so everything
# here is idempotent.
#
# The failure worth catching is silent: with the mount absent these are ordinary
# directories on the host's disk, and every build still succeeds.
set -euo pipefail

RAMDISK="${ROUTARR_RAMDISK:-/ramdisk}"

# The tmpfs is half the RAM, and the build tree outgrows half of less than this.
# A 16 GB machine reports under 16 GiB once firmware and kernel have taken
# their share, so the line sits below it.
MIN_RAM_KB=$((14 * 1024 * 1024))
# On the `routarr-cargo` volume, so a build kept on disk also survives a restart.
DISK_TARGET="${CARGO_HOME:-/cargo}/target"

note() { echo "ramdisk: $*"; }
warn() { echo "ramdisk: $*" >&2; }

# `findmnt` takes an exact mount point, where a substring match on /proc/mounts
# would treat `/ramdisk-old` as a hit.
fstype="$(findmnt -no FSTYPE "$RAMDISK" 2>/dev/null || true)"

if [ "$fstype" != "tmpfs" ]; then
  warn "$RAMDISK is not a tmpfs mount (found: ${fstype:-nothing mounted there})."
  warn "The --mount in devcontainer.json's runArgs did not take effect, so"
  warn "builds will write to the container filesystem, on the host's disk."
  warn "Rebuild the container; if it persists, check that the runtime accepts"
  warn "  --mount type=tmpfs,destination=$RAMDISK,tmpfs-mode=1777"
  # Not fatal: a container that refuses to start over a missing optimisation is
  # worse than a slow one.
  exit 0
fi

# TMPDIR is the mount point itself: processes started before this script inherit
# it, and a TMPDIR that does not exist yet fails `mktemp`.
chmod 1777 "$RAMDISK" 2>/dev/null || true

mem_kb="$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo 2>/dev/null || echo 0)"
if [ "${mem_kb:-0}" -lt "$MIN_RAM_KB" ]; then
  # A link rather than another CARGO_TARGET_DIR: `containerEnv` is fixed when
  # the container is created, and cargo, cargo-llvm-cov and rust-analyzer all
  # follow the link. `rmdir` takes only an empty directory, so a tree already
  # built in RAM is left alone and reported.
  if [ ! -L "$RAMDISK/cargo-target" ]; then
    rmdir "$RAMDISK/cargo-target" 2>/dev/null || true
    mkdir -p "$DISK_TARGET"
    ln -sT "$DISK_TARGET" "$RAMDISK/cargo-target" 2>/dev/null \
      || warn "$RAMDISK/cargo-target holds a build, left in RAM"
  fi
  where="on disk ($DISK_TARGET), the machine has $((mem_kb / 1024)) MiB of RAM"
else
  # Cargo would create this itself, but making it here means a fresh container
  # reports a real path rather than one that appears after the first compile.
  mkdir -p "$RAMDISK/cargo-target"
  where="in RAM"
fi

# `set -e` is on, so the fallbacks keep a failed lookup from taking the script
# down over a line that only prints a figure.
size_mb=$(( $(findmnt -bno SIZE "$RAMDISK" 2>/dev/null || echo 0) / 1024 / 1024 ))
avail_mb=$(( $(findmnt -bno AVAIL "$RAMDISK" 2>/dev/null || echo 0) / 1024 / 1024 ))
note "$RAMDISK is tmpfs, ${size_mb} MiB, ${avail_mb} MiB free, the build tree is $where"
note "CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>}  TMPDIR=${TMPDIR:-<unset>}"
