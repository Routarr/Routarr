#!/usr/bin/env bash
# Run the end-to-end suite against a real Routarr serving the real frontend.
#
# Everything is thrown away afterwards: its own port, its own database, its own
# fake Radarr. Nothing here touches a development instance you may have running.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Resolved, not assumed. The dev container points `CARGO_TARGET_DIR` at a
# ramdisk, so the binary this harness starts is not always under
# `backend/target`, and a hard-coded path fails with "no such file" on a
# machine where the build it depends on has just succeeded. Unset everywhere
# else (CI, a laptop), where the default is the right answer.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/backend/target}"
PORT="${ROUTARR_E2E_PORT:-9877}"
ARR_PORT="${ROUTARR_E2E_ARR_PORT:-7979}"
OIDC_PORT="${ROUTARR_E2E_OIDC_PORT:-7980}"
# The sign-in mode the server runs in. `apikey`, the shipped default, serves
# every spec but those tagged @forms and @oidc, which run against a server in
# their own mode, with a page that holds no key and opens on the sign-in screen.
AUTH="${ROUTARR_E2E_AUTH:-apikey}"
# Named so `prune.sh` sweeps it if this exits without its trap.
WORK="$(mktemp -d -t routarr-e2e-XXXXXX)"

# Wait for each server to actually go away: `kill` only asks. Returning while a
# process still holds the port is what makes the *next* run talk to a corpse.
# Both codes, because which one fires depends on the shellcheck the runner
# image happens to ship: 0.10 calls the body unreachable (SC2317), later
# versions call the function uninvoked (SC2329). Neither can see a trap.
# shellcheck disable=SC2317,SC2329  # invoked through the trap below
cleanup() {
  local pid
  for pid in "${ROUTARR_PID:-}" "${ARR_PID:-}" "${OIDC_PID:-}"; do
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
# run then binds nothing, talks to the stale process instead (whose database
# has just been deleted), and fails in ways that look like application bugs.
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
# Only where it is used: the port may belong to something else otherwise.
if [[ "$AUTH" == oidc ]]; then free_port "$OIDC_PORT"; fi

echo "==> building"
# Built from inside the crate, not with `--manifest-path` from here. rustup
# resolves `rust-toolchain.toml` from the *working directory*, and this script
# runs in `frontend/`: pointing cargo at the manifest compiles the backend with
# whatever the default toolchain happens to be, which is the one thing that file
# exists to prevent. The subshell is safe: unlike the server below, it is
# waited on rather than backgrounded.
(cd "$ROOT/backend" && cargo build --release >/dev/null)
# Kept aside rather than discarded: svelte-check and Vite report on stdout, so
# a failed build would end the run with nothing on screen to say why.
if ! (cd "$ROOT/frontend" && npm run build >"$WORK/frontend-build.log" 2>&1); then
  echo "the frontend did not build:" >&2
  cat "$WORK/frontend-build.log" >&2
  exit 1
fi
# A release build lands beside the debug tree and a coverage tree in the dev
# container's tmpfs. The sweep is what keeps the next build from ENOSPC.
# `CARGO_TARGET_DIR` is set only there, so CI never runs this.
if [[ -n "${CARGO_TARGET_DIR:-}" && -f "$ROOT/.devcontainer/prune.sh" ]]; then
  bash "$ROOT/.devcontainer/prune.sh" >/dev/null 2>&1 || true
fi

echo "==> fake Radarr on :$ARR_PORT"
ARR_PORT="$ARR_PORT" python3 "$ROOT/frontend/e2e/fake_arr.py" &
ARR_PID=$!

# Authenticated, because that is the shipped posture: with no key set, Routarr
# generates one at first start. Running the suite open would leave the shipped
# configuration (every request carrying a key from the browser's storage) the
# one path nothing exercises end to end.
API_KEY="e2e-key-not-a-secret-but-long-enough"
BROWSER_KEY=""
case "$AUTH" in
  apikey)
    MODE=(ROUTARR_API_KEY="$API_KEY")
    BROWSER_KEY="$API_KEY"
    ;;
  forms)
    # The account and its password are generated at first start, and the
    # password written beside the database, where a spec reads it.
    MODE=(ROUTARR_AUTH=forms)
    ;;
  oidc)
    OIDC_CLIENT="routarr-e2e"
    OIDC_SECRET="e2e-client-not-a-secret"
    echo "==> stand-in provider on :$OIDC_PORT"
    OIDC_PORT="$OIDC_PORT" OIDC_CLIENT_ID="$OIDC_CLIENT" OIDC_CLIENT_SECRET="$OIDC_SECRET" \
      python3 "$ROOT/frontend/e2e/fake_oidc.py" &
    OIDC_PID=$!
    DISCOVERY="http://127.0.0.1:$OIDC_PORT/.well-known/openid-configuration"
    for _ in $(seq 1 50); do
      if curl -sf "$DISCOVERY" >/dev/null; then break; fi
      sleep 0.1
    done
    MODE=(
      ROUTARR_AUTH=oidc
      ROUTARR_OIDC_ISSUER="http://127.0.0.1:$OIDC_PORT"
      ROUTARR_OIDC_CLIENT_ID="$OIDC_CLIENT"
      ROUTARR_OIDC_CLIENT_SECRET="$OIDC_SECRET"
      ROUTARR_OIDC_REDIRECT_URL="http://127.0.0.1:$PORT${ROUTARR_E2E_BASE:-}/api/v1/auth/oidc/callback"
      # The `sub` the stand-in signs in (`SUBJECT` in fake_oidc.py).
      ROUTARR_OIDC_ALLOWED_SUBJECTS=e2e-operator
    )
    ;;
  *)
    echo "ROUTARR_E2E_AUTH is apikey, forms or oidc, not $AUTH" >&2
    exit 1
    ;;
esac

echo "==> Routarr on :$PORT"
# Started directly rather than inside a `( cd … ) &` subshell: there `$!` is the
# subshell's pid, so the trap kills the wrapper and leaves the real server alive,
# holding the port with a database that has just been deleted, which is exactly
# the stale process `free_port` exists to survive. The frontend directory is
# therefore passed explicitly instead of relying on the `./frontend/dist`
# default resolving against a working directory. Through `env`, which replaces
# itself with the server, `$!` stays the pid the trap has to kill.
#
# ROUTARR_E2E_BASE lets the specs tagged @subpath exercise the reverse-proxy
# sub-path against the same harness, rather than needing a second one.
env \
  ROUTARR_DB_PATH="$WORK/routarr.db" \
  ROUTARR_PORT="$PORT" \
  ROUTARR_FRONTEND_DIR="$ROOT/frontend/dist" \
  ROUTARR_BASE_PATH="${ROUTARR_E2E_BASE:-}" \
  "${MODE[@]}" \
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
ROUTARR_E2E_KEY="$BROWSER_KEY" \
ROUTARR_E2E_PASSWORD_FILE="$WORK/routarr.password" \
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
