#!/usr/bin/env bash
# Prepare the tmpfs ramdisk, and say so loudly if it is not one.
#
# The mount is declared in `devcontainer.json`'s `runArgs` and made by the
# container runtime; this only prepares what lives inside it. Runs on create and
# on every start, since a tmpfs is empty again after each, so everything here is
# idempotent.
#
# The failure worth catching is silent: with the mount absent these are ordinary
# directories on the host's disk, and every build still succeeds.
set -euo pipefail

RAMDISK="${ROUTARR_RAMDISK:-/ramdisk}"

# Read out of the declaration rather than restated here. Written twice, the two
# drift the first time one of them is raised — and the failure is silent in the
# worse direction: an assertion still comparing against the old figure warns on
# every start about a mount that is exactly what was asked for, which teaches
# the reader to ignore it. `devcontainer.json` carries `//` comments, so this is
# a grep and not `jq`. The fallback is only for a copy of this script running
# away from the file.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WANT_BYTES="$(grep -o 'tmpfs-size=[0-9]\+' "$HERE/devcontainer.json" 2>/dev/null | head -1 | cut -d= -f2)"
WANT_BYTES="${WANT_BYTES:-$((16 * 1024 * 1024 * 1024))}"

note() { echo "ramdisk: $*"; }
warn() { echo "ramdisk: $*" >&2; }

# `findmnt` takes an exact mount point; a substring match on /proc/mounts would
# treat `/ramdisk-old` as a hit.
fstype="$(findmnt -no FSTYPE "$RAMDISK" 2>/dev/null || true)"

if [ "$fstype" != "tmpfs" ]; then
  warn "$RAMDISK is not a tmpfs mount (found: ${fstype:-nothing mounted there})."
  warn "The --mount in devcontainer.json's runArgs did not take effect, so"
  warn "builds will write to the container filesystem — on the host's disk."
  warn "Rebuild the container; if it persists, check that the runtime accepts"
  warn "  --mount type=tmpfs,destination=$RAMDISK,tmpfs-size=$WANT_BYTES"
  # Not fatal: a container that refuses to start over a missing optimisation is
  # worse than a slow one.
  exit 0
fi

size_bytes="$(findmnt -bno SIZE "$RAMDISK" 2>/dev/null || echo 0)"
if [ "$size_bytes" -ne "$WANT_BYTES" ]; then
  warn "$RAMDISK is tmpfs but $((size_bytes / 1024 / 1024)) MiB, not the"
  warn "$((WANT_BYTES / 1024 / 1024)) MiB declared in devcontainer.json."
fi

# Cargo would create this itself; making it here means a fresh container reports
# a real path rather than one that appears after the first compile.
mkdir -p "$RAMDISK/cargo-target"

# TMPDIR is the mount point itself: processes started before this script inherit
# it, and a TMPDIR that does not exist yet fails `mktemp`.
chmod 1777 "$RAMDISK" 2>/dev/null || true

# `set -e` is on, so the fallback keeps a failed lookup from taking the script
# down over a line that only prints a figure.
avail_mb=$(( $(findmnt -bno AVAIL "$RAMDISK" 2>/dev/null || echo 0) / 1024 / 1024 ))
note "$RAMDISK is tmpfs, $((size_bytes / 1024 / 1024)) MiB, ${avail_mb} MiB free"
note "CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>}  TMPDIR=${TMPDIR:-<unset>}"
