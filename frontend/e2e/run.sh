#!/usr/bin/env bash
# Run the end-to-end suite against a real Routarr serving the real frontend.
#
# Everything is thrown away afterwards: its own port, its own database, its own
# fake Radarr. Nothing here touches a development instance you may have running.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Resolved, not assumed. The dev container points `CARGO_TARGET_DIR` at a
# ramdisk, so the binary this harness starts is not always under
# `backend/target` — and a hard-coded path fails with "no such file" on a
# machine where the build it depends on has just succeeded. Unset everywhere
# else (CI, a laptop), where the default is the right answer.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/backend/target}"
PORT="${ROUTARR_E2E_PORT:-9877}"
ARR_PORT="${ROUTARR_E2E_ARR_PORT:-7979}"
# Named so `prune.sh` sweeps it if this exits without its trap.
WORK="$(mktemp -d -t routarr-e2e-XXXXXX)"

# Wait for each server to actually go away: `kill` only asks. Returning while a
# process still holds the port is what makes the *next* run talk to a corpse.
# shellcheck disable=SC2329  # invoked through the trap below
cleanup() {
  local pid
  for pid in "${ROUTARR_PID:-}" "${ARR_PID:-}"; do
    [[ -z "$pid" ]] && continue
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$pid" 2>/dev/null || true
  done
  rm -rf "$WORK"
}
trap cleanup EXIT

# A run killed before its trap fires leaves a server holding the port. The next
# run then binds nothing, talks to the stale process instead — whose database
# has just been deleted — and fails in ways that look like application bugs.
free_port() {
  local port=$1 pid
  for pid in $(fuser -n tcp "$port" 2>/dev/null || true); do
    echo "    freeing :$port (stale pid $pid)"
    kill "$pid" 2>/dev/null || true
  done
  # Give the kernel a moment to release the socket.
  for _ in $(seq 1 20); do
    fuser -n tcp "$port" >/dev/null 2>&1 || return 0
    sleep 0.1
  done
  echo "port $port is still busy" >&2
  exit 1
}

echo "==> freeing ports"
free_port "$PORT"
free_port "$ARR_PORT"

echo "==> building"
# Built from inside the crate, not with `--manifest-path` from here. rustup
# resolves `rust-toolchain.toml` from the *working directory*, and this script
# runs in `frontend/`: pointing cargo at the manifest compiles the backend with
# whatever the default toolchain happens to be, which is the one thing that file
# exists to prevent. The subshell is safe — unlike the server below, this is
# waited on rather than backgrounded.
(cd "$ROOT/backend" && cargo build --release >/dev/null)
(cd "$ROOT/frontend" && npm run build >/dev/null)
# A release build lands beside the debug tree and a coverage tree in the dev
# container's tmpfs; the sweep is what keeps the next build from ENOSPC.
# `CARGO_TARGET_DIR` is set only there, so CI never runs this.
if [[ -n "${CARGO_TARGET_DIR:-}" && -f "$ROOT/.devcontainer/prune.sh" ]]; then
  bash "$ROOT/.devcontainer/prune.sh" >/dev/null 2>&1 || true
fi

echo "==> fake Radarr on :$ARR_PORT"
ARR_PORT="$ARR_PORT" python3 "$ROOT/frontend/e2e/fake_arr.py" &
ARR_PID=$!

echo "==> Routarr on :$PORT"
# Started directly rather than inside a `( cd … ) &` subshell: there `$!` is the
# subshell's pid, so the trap kills the wrapper and leaves the real server alive,
# holding the port with a database that has just been deleted — exactly the
# stale process `free_port` exists to survive. The frontend directory is
# therefore passed explicitly instead of relying on the `./frontend/dist`
# default resolving against a working directory.
#
# ROUTARR_E2E_BASE lets one spec exercise the reverse-proxy sub-path against the
# same harness, rather than needing a second one.
# Authenticated, because that is the shipped posture: with no key set, Routarr
# generates one at first start. Running the suite open would leave the
# shipped configuration — every request carrying a key from the browser's
# storage — the one path nothing exercises end to end.
API_KEY="e2e-key-not-a-secret"

ROUTARR_DB_PATH="$WORK/routarr.db" \
ROUTARR_PORT="$PORT" \
ROUTARR_FRONTEND_DIR="$ROOT/frontend/dist" \
ROUTARR_BASE_PATH="${ROUTARR_E2E_BASE:-}" \
ROUTARR_API_KEY="$API_KEY" \
  "$TARGET_DIR/release/routarr" >"$WORK/routarr.log" 2>&1 &
ROUTARR_PID=$!

PING="http://127.0.0.1:$PORT${ROUTARR_E2E_BASE:-}/api/v1/ping"
for _ in $(seq 1 50); do
  if curl -sf "$PING" >/dev/null; then break; fi
  sleep 0.2
done
curl -sf "$PING" >/dev/null || {
  echo "Routarr did not start:"; cat "$WORK/routarr.log"; exit 1;
}

echo "==> playwright"
cd "$ROOT/frontend"
set +e
ROUTARR_E2E_URL="http://127.0.0.1:$PORT${ROUTARR_E2E_BASE:-}" \
ROUTARR_E2E_ARR="http://127.0.0.1:$ARR_PORT" \
ROUTARR_E2E_KEY="$API_KEY" \
  npx playwright test "$@"
STATUS=$?
set -e

# A browser-side failure is often a server-side one. Without this the log dies
# with the temporary directory and the cause is invisible.
if [[ $STATUS -ne 0 ]]; then
  echo
  echo "==> server log (last 40 lines)"
  tail -40 "$WORK/routarr.log"
fi

exit $STATUS
