#!/usr/bin/env bash
# Prepare the tmpfs ramdisk, and warn loudly when the mount is missing: builds then
# write to the host's disk and still succeed.
#
# The mount itself comes from `runArgs` in `devcontainer.json`. This runs on create and on
# every start, since a tmpfs comes back empty, so every step is idempotent.
set -euo pipefail

RAMDISK="${ROUTARR_RAMDISK:-/ramdisk}"

# The tmpfs gets half the RAM, too little for the build tree below this. A 16 GB machine
# reports under 16 GiB once firmware and kernel take their share.
MIN_RAM_KB=$((14 * 1024 * 1024))
# On the `routarr-cargo` volume, so a build kept on disk survives a restart.
DISK_TARGET="${CARGO_HOME:-/cargo}/target"

note() { echo "ramdisk: $*"; }
warn() { echo "ramdisk: $*" >&2; }

# `findmnt` matches the exact mount point, where a grep of /proc/mounts takes `/ramdisk-old`.
fstype="$(findmnt -no FSTYPE "$RAMDISK" 2>/dev/null || true)"

if [ "$fstype" != "tmpfs" ]; then
  warn "$RAMDISK is not a tmpfs mount (found: ${fstype:-nothing mounted there})."
  warn "The --mount in devcontainer.json's runArgs did not take effect, so"
  warn "builds will write to the container filesystem, on the host's disk."
  warn "Rebuild the container; if it persists, check that the runtime accepts"
  warn "  --mount type=tmpfs,destination=$RAMDISK,tmpfs-mode=1777"
  # Not fatal: a slow container is better than one that refuses to start.
  exit 0
fi

# TMPDIR is the mount point itself (see `devcontainer.json`), so it needs the mode of /tmp.
chmod 1777 "$RAMDISK" 2>/dev/null || true

mem_kb="$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo 2>/dev/null || echo 0)"
if [ "${mem_kb:-0}" -lt "$MIN_RAM_KB" ]; then
  # A link, since `containerEnv` fixes CARGO_TARGET_DIR at creation, and cargo,
  # cargo-llvm-cov and rust-analyzer all follow it. `rmdir` removes only an empty
  # directory, so a tree already built in RAM stays there.
  if [ ! -L "$RAMDISK/cargo-target" ]; then
    rmdir "$RAMDISK/cargo-target" 2>/dev/null || true
    mkdir -p "$DISK_TARGET"
    ln -sT "$DISK_TARGET" "$RAMDISK/cargo-target" 2>/dev/null \
      || warn "$RAMDISK/cargo-target holds a build, left in RAM"
  fi
  where="on disk ($DISK_TARGET), the machine has $((mem_kb / 1024)) MiB of RAM"
else
  # Cargo would create it at the first compile. Made here, the path exists from the start.
  mkdir -p "$RAMDISK/cargo-target"
  where="in RAM"
fi

# The fallbacks keep `set -e` from ending the script over a line that only prints sizes.
size_mb=$(( $(findmnt -bno SIZE "$RAMDISK" 2>/dev/null || echo 0) / 1024 / 1024 ))
avail_mb=$(( $(findmnt -bno AVAIL "$RAMDISK" 2>/dev/null || echo 0) / 1024 / 1024 ))
note "$RAMDISK is tmpfs, ${size_mb} MiB, ${avail_mb} MiB free, the build tree is $where"
note "CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>}  TMPDIR=${TMPDIR:-<unset>}"
