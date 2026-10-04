#!/usr/bin/env bash
# Start a built image the way the README runs it, and check that it serves.
#
# A green build says the Dockerfile compiles. It says nothing about whether the
# binary starts on the runtime base, whether uid 1000 can write its volume,
# whether the healthcheck reaches the port it names, whether the frontend was
# copied in, or whether the key is where the first-run screen tells people to
# look. Each of those can fail while the build passes.
#
# Usage: bash scripts/smoke-image.sh <image>
# Needs Docker. CI runs it after the image job's build. The host port is
# SMOKE_PORT, 9876 by default: set it when a development Routarr holds that one.
# SMOKE_RUNTIME names a runtime registered with Docker, `runsc` for gVisor, and
# every container the script starts then runs under it.
set -euo pipefail

IMAGE="${1:?usage: smoke-image.sh <image>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${SMOKE_PORT:-9876}"
RUNTIME="${SMOKE_RUNTIME:-}"
# Expanded as `${RUNTIME_FLAGS[@]+...}`: under `set -u`, bash before 4.4
# (macOS ships 3.2) calls an empty array unbound.
RUNTIME_FLAGS=()
[ -z "$RUNTIME" ] || RUNTIME_FLAGS=(--runtime "$RUNTIME")

# The container name the README and docker-compose.yml use, which the command
# on the first-run screen relies on.
NAME=routarr
VOLUME=routarr-smoke-data
SUBPATH_NAME=routarr-smoke-subpath
BASE="http://127.0.0.1:$PORT"

fail() {
  echo "::error::$*" >&2
  docker logs "$NAME" 2>&1 | tail -40 >&2 || true
  exit 1
}
ok() { echo "ok   $*"; }

# The name is the one a real installation uses, so an existing container may be
# somebody's Routarr: refuse rather than remove it, and remove only what this
# run created.
for existing in "container:$NAME" "volume:$VOLUME" "container:$SUBPATH_NAME"; do
  if docker "${existing%%:*}" inspect "${existing#*:}" >/dev/null 2>&1; then
    echo "::error::a ${existing%%:*} named ${existing#*:} already exists; not touching it" >&2
    exit 1
  fi
done
cleanup() {
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker rm -fv "$SUBPATH_NAME" >/dev/null 2>&1 || true
  docker volume rm -f "$VOLUME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

http_status() { curl -s --max-time 10 -o /dev/null -w '%{http_code}' "$@"; }

# Gives up as soon as the container has exited: otherwise a crash on start
# reads as a slow start for the whole timeout.
wait_for() {
  local seconds=$1 what=$2
  shift 2
  local deadline=$((SECONDS + seconds))
  until "$@"; do
    [ "$(docker inspect -f '{{.State.Running}}' "$NAME")" = true ] ||
      fail "the container exited while waiting for $what"
    [ "$SECONDS" -lt "$deadline" ] || fail "no $what after ${seconds}s"
    sleep 1
  done
}
pings() { [ "$(http_status "$BASE/api/v1/ping")" = 200 ]; }
# Captured whole rather than piped into `grep -q`: grep exits on the first
# match, `docker logs` takes a SIGPIPE, and under pipefail a line that is there
# reads as a line that is not.
logs() { docker logs "$NAME" 2>&1; }
healthy() { [ "$(docker inspect -f '{{.State.Health.Status}}' "$NAME")" = healthy ]; }

expect_status() {
  local want=$1 what=$2
  shift 2
  local got
  got=$(http_status "$@")
  [ "$got" = "$want" ] || fail "$what: expected HTTP $want, got $got"
  ok "$what"
}

# The command the first-run screen prints, read from the screen itself, so the
# instruction a new user follows is the one this runs.
read_key=$(sed -n "s/^ *const READ_KEY_COMMAND = '\(.*\)';$/\1/p" \
  "$ROOT/frontend/src/components/ApiKeyGate.svelte")
[ -n "$read_key" ] || fail "READ_KEY_COMMAND not found in ApiKeyGate.svelte"

# The image runs here without the compose file, which names the container and
# the data path for everyone who follows the README: the command has to name
# the same two.
compose_name=$(sed -n 's/^ *container_name: *//p' "$ROOT/docker-compose.yml")
compose_db=$(sed -n 's/^ *- ROUTARR_DB_PATH=//p' "$ROOT/docker-compose.yml")
[ "$compose_name" = "$NAME" ] ||
  fail "docker-compose.yml names the container '$compose_name', the first-run command '$NAME'"
[ "$read_key" = "docker exec $compose_name cat ${compose_db%/*}/routarr.api_key" ] ||
  fail "\`$read_key\` does not read the key docker-compose.yml's ROUTARR_DB_PATH=$compose_db puts beside the database"
ok "the first-run command names the container and the data path of docker-compose.yml"

# A named volume, fresh, with the hardening the README's compose file sets.
docker run -d --name "$NAME" ${RUNTIME_FLAGS[@]+"${RUNTIME_FLAGS[@]}"} \
  -p "127.0.0.1:$PORT:9876" \
  -v "$VOLUME:/data" \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  "$IMAGE" >/dev/null

wait_for 30 "answer on /api/v1/ping" pings
ok "starts and answers /api/v1/ping without a key"

# The point of an isolating runtime is that the container no longer runs on
# the host's kernel: gVisor answers with a kernel of its own.
if [ -n "$RUNTIME" ]; then
  inner=$(docker exec "$NAME" uname -r) || fail "cannot read the kernel the container sees"
  [ "$inner" != "$(uname -r)" ] || fail "under $RUNTIME the container still sees the host's kernel, $inner"
  ok "runs under $RUNTIME, on a kernel that is not the host's ($inner)"
fi

uid=$(docker exec "$NAME" awk '/^Uid:/ { print $2 }' /proc/1/status) || fail "cannot read PID 1's uid"
[ "$uid" = 1000 ] || fail "the server runs as uid $uid, not 1000"
ok "runs as uid 1000"

expect_status 401 "refuses /api/v1/status without a key" "$BASE/api/v1/status"

# Split into words on purpose: it is a command line with no quoting in it.
# shellcheck disable=SC2086
key=$($read_key) || fail "\`$read_key\` failed"
[[ "$key" =~ ^[0-9a-f]{64}$ ]] || fail "\`$read_key\` printed something other than a key"
ok "\`$read_key\` prints the generated key"

grep -qF "$key" <<<"$(logs)" || fail "the generated key is not in the log"
ok "the generated key is in the log"

expect_status 200 "accepts that key on /api/v1/status" -H "X-Api-Key: $key" "$BASE/api/v1/status"

# The frontend, served with the mount point the backend injects. Without the
# copy in the Dockerfile the server still starts, in API-only mode.
index=$(curl -fsS --max-time 10 "$BASE/") || fail "GET / failed"
grep -qF '<base href="/">' <<<"$index" || fail "GET / carries no <base href>"
entry=$(sed -n 's/.*<script[^>]* src="\.\/\(assets\/[^"]*\.js\)".*/\1/p' <<<"$index")
[ -n "$entry" ] || fail "GET / references no entry script"
expect_status 200 "serves the interface and its entry script" "$BASE/$entry"

# GPLv3 §4 and §6: the licence travels with the program, and the image is how
# almost everyone receives it.
docker exec "$NAME" test -s /app/LICENSE || fail "no LICENSE in the image"
ok "carries the licence"

# reqwest verifies TLS against the platform's store: without the bundle, every
# metadata source and every Arr reached over HTTPS fails certificate checks.
docker exec "$NAME" test -s /etc/ssl/certs/ca-certificates.crt || fail "no CA bundle in the image"
ok "carries the CA bundle"

# Nothing under /app is written at run time, and a process that can rewrite its
# binary or the page it serves turns any code execution into a lasting one.
if docker exec "$NAME" sh -c 'test -w /app || test -w /app/routarr || test -w /app/frontend/dist/index.html'; then
  fail "uid 1000 can write under /app"
fi
ok "leaves /app read-only to uid 1000"

# The way back in for a locked-out operator, run where the README says, with
# the server still running: the image carries no sqlite3 to do it by hand.
reset=$(docker exec "$NAME" /app/routarr reset-account) || fail "reset-account failed"
grep -qF 'The account is reset' <<<"$reset" || fail "reset-account printed no new password"
docker exec "$NAME" test -s /data/routarr.password || fail "reset-account wrote no password file"
ok "\`/app/routarr reset-account\` gives the account a new password"

# Docker's first probe runs one interval (30s) after start.
wait_for 60 "healthy status from the HEALTHCHECK" healthy
ok "the HEALTHCHECK reports healthy"

# The mount point as people often write it, with no slash. The probe reads it
# as the server does, and only the API's own answer counts, never the page the
# interface falls back to.
docker run -d --name "$SUBPATH_NAME" ${RUNTIME_FLAGS[@]+"${RUNTIME_FLAGS[@]}"} -e ROUTARR_BASE_PATH=routarr \
  --health-interval=2s --health-start-period=1s "$IMAGE" >/dev/null
deadline=$((SECONDS + 60))
until [ "$(docker inspect -f '{{.State.Health.Status}}' "$SUBPATH_NAME")" = healthy ]; do
  [ "$SECONDS" -lt "$deadline" ] || fail "ROUTARR_BASE_PATH=routarr never became healthy"
  sleep 1
done
docker rm -fv "$SUBPATH_NAME" >/dev/null
ok "the HEALTHCHECK follows ROUTARR_BASE_PATH=routarr"

# PID 1 ignores a signal it installed no handler for, so a server that misses
# SIGTERM is killed after ten seconds with 137, and skips the checkpoint that
# writes the WAL back into the database.
docker stop "$NAME" >/dev/null
code=$(docker inspect -f '{{.State.ExitCode}}' "$NAME")
[ "$code" = 0 ] || fail "docker stop ended with exit code $code, not a clean shutdown"
grep -qF 'Routarr stopped cleanly' <<<"$(logs)" || fail "no clean-shutdown line in the log"
ok "stops cleanly on SIGTERM"

docker start "$NAME" >/dev/null
wait_for 30 "answer on /api/v1/ping after a restart" pings
expect_status 200 "still accepts the same key after a restart" -H "X-Api-Key: $key" "$BASE/api/v1/status"
generated=$(grep -cF 'Generated an API key' <<<"$(logs)" || true)
[ "$generated" = 1 ] || fail "a key was generated $generated times across two starts"
ok "does not generate a second key on restart"

echo "The image starts, serves, and keeps its key."
