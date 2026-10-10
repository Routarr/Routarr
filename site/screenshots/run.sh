#!/usr/bin/env bash
# Render the README's screenshot of the simulation from the real application,
# into .github/assets/simulation.webp.
#
# Everything is disposable: its own database, its own fake Radarr, Sonarr and
# TMDB, its own ports. A development instance is never touched, and no real
# library or API key can end up in a published image.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Resolved, not assumed: the dev container points `CARGO_TARGET_DIR` at a
# ramdisk, so the binary is not always under `backend/target`. Unset everywhere
# else, where the default is the right answer.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/backend/target}"
HERE="$ROOT/site/screenshots"
OUT="$ROOT/.github/assets/simulation.webp"
# Named so `prune.sh` sweeps it if this exits without its trap.
WORK="$(mktemp -d -t routarr-shots-XXXXXX)"
# Both generated per run: the instance is thrown away with its database, so a
# key written into this file would be a secret in the repository that guards
# nothing, and the one thing a secret scanner is right to refuse.
DEMO_API_KEY="$(openssl rand -hex 32)"
DEMO_SECRET_KEY="$(openssl rand -base64 32)"

# A port nothing holds, asked of the kernel: a fixed one collides with an e2e
# run or a second checkout, and freeing it would kill whatever holds it.
free_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}
PORT="$(free_port)"
RADARR_PORT="$(free_port)"
SONARR_PORT="$(free_port)"
TMDB_PORT="$(free_port)"

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

echo "==> building"
# From inside the crate: rustup reads `rust-toolchain.toml` from the working
# directory, so `--manifest-path` from here would compile with whatever the
# default toolchain is rather than the pinned one.
(cd "$ROOT/backend" && cargo build --release --locked >/dev/null)
# svelte-check's report is the reason a build fails, so it is kept and shown.
(cd "$ROOT/frontend" && npm run build >"$WORK/frontend-build.log" 2>&1) || {
  cat "$WORK/frontend-build.log" >&2
  exit 1
}

echo "==> fakes"
ARR_MODE=radarr ARR_PORT="$RADARR_PORT" python3 "$HERE/fake_arr.py" & PIDS+=($!)
ARR_MODE=sonarr ARR_PORT="$SONARR_PORT" python3 "$HERE/fake_arr.py" & PIDS+=($!)
TMDB_PORT="$TMDB_PORT" python3 "$HERE/fake_tmdb.py" & PIDS+=($!)
wait_for "http://127.0.0.1:$RADARR_PORT/api/v3/system/status"
wait_for "http://127.0.0.1:$SONARR_PORT/api/v3/system/status"
wait_for "http://127.0.0.1:$TMDB_PORT/3/configuration"

echo "==> seeding"
start_routarr
ROUTARR_URL="http://127.0.0.1:$PORT" ROUTARR_API_KEY="$DEMO_API_KEY" \
  RADARR_PORT="$RADARR_PORT" SONARR_PORT="$SONARR_PORT" node "$HERE/seed.mjs"

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
# Captured without a decision, the screen would show its empty state and the
# README a picture of an application that does nothing.
[[ "${total:-0}" -gt 0 ]] || { echo "no decision after 40 s; not capturing" >&2; exit 1; }

echo "==> capturing"
FRONTEND_DIR="$ROOT/frontend" ROUTARR_URL="http://127.0.0.1:$PORT" \
  ROUTARR_API_KEY="$DEMO_API_KEY" \
  SHOT_PNG="$WORK/simulation.png" node "$HERE/capture.mjs"

# Encoded in the work directory and moved in once whole: written in place, a
# failed encode would leave the README a broken image.
convert "$WORK/simulation.png" -quality 82 -define webp:method=6 "$WORK/simulation.webp"
mv "$WORK/simulation.webp" "$OUT"
ls -la "$OUT"
