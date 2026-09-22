#!/usr/bin/env bash
# Render the showcase's captured images from the real application.
#
# The pages show no screenshot since the site was cut to two, so what this
# produces today is the Open Graph card, and the captures are kept ready for
# the second page. Everything is disposable: its own database, its own fake Radarr, Sonarr and
# TMDb, its own ports. A development instance is never touched, and no real
# library or API key can end up in a published image.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Resolved, not assumed: the dev container points `CARGO_TARGET_DIR` at a
# ramdisk, so the binary is not always under `backend/target`. Unset everywhere
# else, where the default is the right answer.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/backend/target}"
HERE="$ROOT/site/screenshots"
PORT="${SHOT_PORT:-9899}"
# Not the e2e harness's ports (9877, 7979): both may run at once, and each
# kills whatever holds its port.
RADARR_PORT=7989
SONARR_PORT=7991
TMDB_PORT=7990
# Named so `prune.sh` sweeps it if this exits without its trap.
WORK="$(mktemp -d -t routarr-shots-XXXXXX)"
# Throwaway, and never shown: the interface masks stored keys.
DEMO_API_KEY="showcase-only-api-key"
# Generated per run: the instance is thrown away with its database, so a key
# written into this file was a secret in the repository that guarded nothing —
# and the one thing a secret scanner is right to refuse.
DEMO_SECRET_KEY="$(openssl rand -base64 32)"

PIDS=()
cleanup() {
  local pid
  for pid in "${PIDS[@]:-}"; do
    [[ -z "$pid" ]] && continue
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
    kill -9 "$pid" 2>/dev/null || true
  done
  rm -rf "$WORK"
}
trap cleanup EXIT

free_port() {
  local pid
  for pid in $(fuser -n tcp "$1" 2>/dev/null || true); do kill -9 "$pid" 2>/dev/null || true; done
}

wait_for() {
  local url=$1
  for _ in $(seq 1 60); do curl -sf "$url" >/dev/null && return 0; sleep 0.25; done
  echo "timed out waiting for $url" >&2
  return 1
}

start_routarr() {
  ROUTARR_DB_PATH="$WORK/routarr.db" \
  ROUTARR_PORT="$PORT" \
  ROUTARR_FRONTEND_DIR="$ROOT/frontend/dist" \
  ROUTARR_SECRET_KEY="$DEMO_SECRET_KEY" \
  ROUTARR_API_KEY="$DEMO_API_KEY" \
  TMDB_API_KEY="demo" \
  ROUTARR_TMDB_BASE_URL="http://127.0.0.1:$TMDB_PORT/3" \
    "$TARGET_DIR/release/routarr" >>"$WORK/routarr.log" 2>&1 &
  ROUTARR_PID=$!
  PIDS+=("$ROUTARR_PID")
  wait_for "http://127.0.0.1:$PORT/api/v1/ping"
}

echo "==> freeing ports"
for p in "$PORT" "$RADARR_PORT" "$SONARR_PORT" "$TMDB_PORT"; do free_port "$p"; done

echo "==> building"
# From inside the crate: rustup reads `rust-toolchain.toml` from the working
# directory, so `--manifest-path` from here would compile with whatever the
# default toolchain is rather than the pinned one.
(cd "$ROOT/backend" && cargo build --release >/dev/null)
(cd "$ROOT/frontend" && npm run build >/dev/null)

echo "==> fakes"
ARR_MODE=radarr ARR_PORT="$RADARR_PORT" python3 "$HERE/fake_arr.py" & PIDS+=($!)
ARR_MODE=sonarr ARR_PORT="$SONARR_PORT" python3 "$HERE/fake_arr.py" & PIDS+=($!)
TMDB_PORT="$TMDB_PORT" python3 "$HERE/fake_tmdb.py" & PIDS+=($!)
wait_for "http://127.0.0.1:$RADARR_PORT/api/v3/system/status"
wait_for "http://127.0.0.1:$SONARR_PORT/api/v3/system/status"
wait_for "http://127.0.0.1:$TMDB_PORT/3/configuration"

echo "==> seeding"
start_routarr
ROUTARR_URL="http://127.0.0.1:$PORT" ROUTARR_API_KEY="$DEMO_API_KEY" node "$HERE/seed.mjs"

# Restarting is what makes this deterministic: the scheduler sweeps five
# seconds after start-up, and only then does it enrich and simulate. Seeding
# into an already-running instance would leave that to the next 15-minute tick.
echo "==> restarting so the scheduler enriches and simulates"
kill "$ROUTARR_PID"; while kill -0 "$ROUTARR_PID" 2>/dev/null; do sleep 0.1; done
start_routarr

echo "==> waiting for the first pass"
for _ in $(seq 1 80); do
  total=$(curl -sf -H "x-api-key: $DEMO_API_KEY" "http://127.0.0.1:$PORT/api/v1/decisions?per_page=1" | sed -n 's/.*"total":\([0-9]*\).*/\1/p')
  [[ "${total:-0}" -gt 0 ]] && break
  sleep 0.5
done
echo "    ${total:-0} decision(s)"
# Captured without a decision, every screen would show its empty state and the
# site would ship pictures of an application that does nothing.
[[ "${total:-0}" -gt 0 ]] || { echo "no decision after 40 s; not capturing" >&2; exit 1; }

echo "==> capturing"
FRONTEND_DIR="$ROOT/frontend" ROUTARR_URL="http://127.0.0.1:$PORT" \
  ROUTARR_API_KEY="$DEMO_API_KEY" \
  SHOTS_DIR="$ROOT/site/public/assets/shots" node "$HERE/capture.mjs"

# Two formats from the same PNG, never one from the other: re-encoding a lossy
# image into another lossy format compounds both sets of artefacts, and these
# are screenshots of text, where that shows first.
#
# AVIF is offered above WebP in the markup and every browser that cannot read it
# simply takes the next <source>. It is worth the second file here because the
# page is mostly screenshots — roughly a third off the only heavy thing on it.
#
# `avifenc` rather than ImageMagick for the AVIF: an ImageMagick without the
# delegate does not fail, it writes a PNG under the .avif name — a file the
# browser refuses and nothing downstream notices, since the name is right and
# the pair is complete. Encoding through the tool that only does AVIF removes
# the possibility.
echo "==> encoding to avif and webp"
command -v avifenc >/dev/null || {
  echo "avifenc is missing: install libavif-bin. Refusing to write a PNG named .avif." >&2
  exit 1
}
# Encoded in the work directory and moved in as a pair: written in place, a
# failure between the two left a fresh WebP beside a stale AVIF, and the AVIF
# is the file most browsers take.
mkdir -p "$WORK/encoded"
for png in "$ROOT"/site/public/assets/shots/*.png; do
  [[ -e "$png" ]] || continue
  name="$(basename "${png%.png}")"
  convert "$png" -quality 82 -define webp:method=6 "$WORK/encoded/$name.webp"
  # -s 4 is the speed/size middle ground; -q 60 matches the WebP's weight
  # class, and these are pictures of text where banding shows first.
  avifenc --min 0 --max 63 -a end-usage=q -a cq-level=28 -s 4 "$png" "$WORK/encoded/$name.avif" >/dev/null
  mv "$WORK/encoded/$name.webp" "$WORK/encoded/$name.avif" "$ROOT/site/public/assets/shots/"
  rm -f "$png"
done

# What a wrong encoder produced silently until now.
for avif in "$ROOT"/site/public/assets/shots/*.avif; do
  file "$avif" | grep -q "AVIF" || {
    echo "$avif is not an AVIF file" >&2
    exit 1
  }
done
ls -la "$ROOT/site/public/assets/shots"
