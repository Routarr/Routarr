#!/usr/bin/env bash
# Whether the pinned API contract breaks a client of the last release.
#
#   scripts/check-api-breaks.sh
#
# Compares backend/openapi/v1.json with the same file at the newest release tag,
# through oasdiff: an operation, a parameter or a field removed or renamed, a
# type changed, an input made required, a value no longer accepted. Anything
# added passes. From 1.0 a break belongs in `/api/v2`, never in v1. Before it,
# a minor release may break v1: each break it accepts is a line of
# `scripts/api-breaks/since-<tag>.txt`, named in its release notes, and the
# list goes once the next tag is pushed.
#
# It needs the tags and their history (`fetch-depth: 0` in CI). A release that
# predates the pinned contract has nothing to compare with, and passes.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONTRACT=backend/openapi/v1.json

# Releases only: version sorting ranks `v1.2.0-rc.1` above `v1.2.0`, and a
# client of the release is what the contract promises not to break.
tag=$(git -C "$ROOT" tag --list 'v*' --sort=-version:refname | awk '!/-/' | sed -n 1p)
if [ -z "$tag" ]; then
  echo "No release tag: nothing to compare the contract with."
  exit 0
fi

base=$(mktemp)
trap 'rm -f "$base"' EXIT
if ! git -C "$ROOT" show "$tag:$CONTRACT" > "$base" 2>/dev/null; then
  echo "$tag has no pinned contract: nothing to compare with."
  exit 0
fi

# A list kept past its release would wave through the next one's breaks
# unread, and from 1.0 there is none to keep.
accepted="$ROOT/scripts/api-breaks/since-$tag.txt"
for list in "$ROOT"/scripts/api-breaks/since-*.txt; do
  [ -e "$list" ] || continue
  if [ "$list" != "$accepted" ]; then
    echo "error: ${list#"$ROOT"/} lists the breaks of a release already tagged. Delete it."
    exit 1
  fi
  case "$tag" in
    v0.*) ;;
    *)
      echo "error: ${list#"$ROOT"/} accepts breaks of v1 after 1.0. They belong in /api/v2."
      exit 1
      ;;
  esac
done
ignore=()
if [ -e "$accepted" ]; then
  ignore=(--err-ignore "$accepted")
  echo "Breaks accepted for this release: ${accepted#"$ROOT"/}"
fi

echo "Comparing $CONTRACT with $tag"
# oasdiff reads no extension, and `x-routarr-scope` is part of what a key was
# given: raised, it refuses a client the release let in, and lowered, it hands
# every key already issued a reach nobody granted it.
python3 - "$base" "$ROOT/$CONTRACT" <<'PY'
import json
import sys


def scopes(path):
    with open(path) as contract:
        paths = json.load(contract).get("paths", {})
    return {
        f"{method.upper()} {route}": operation.get("x-routarr-scope")
        for route, item in paths.items()
        for method, operation in item.items()
        if isinstance(operation, dict)
    }


released, current = scopes(sys.argv[1]), scopes(sys.argv[2])
changed = [
    f"{operation}: {scope} in the release, {current[operation]} now"
    for operation, scope in sorted(released.items())
    if scope and operation in current and current[operation] != scope
]
for line in changed:
    print(f"error: x-routarr-scope changed on {line}")

# The notification webhook receives a `Notification`, and no operation returns
# one, so oasdiff never compares it. A field it loses is a receiver broken.
PAYLOADS = ["Notification"]


def fields(path, schema):
    with open(path) as contract:
        schemas = json.load(contract).get("components", {}).get("schemas", {})
    return set(schemas.get(schema, {}).get("properties", {}))


removed = [
    f"{schema}.{field}"
    for schema in PAYLOADS
    for field in sorted(fields(sys.argv[1], schema) - fields(sys.argv[2], schema))
]
for field in removed:
    print(f"error: {field} is in the release's contract and no longer documented")
sys.exit(1 if changed or removed else 0)
PY
# oasdiff grades a removed optional field as information, since a client may
# not read it. The contract promises no documented field disappears, so
# `oasdiff-levels.txt` raises that one to an error. A type whose fields another
# one adds to (`#[serde(flatten)]`) is published as `allOf`, and compared
# branch by branch, every field of it reads as removed.
oasdiff breaking "$base" "$ROOT/$CONTRACT" \
  --severity-levels "$ROOT/scripts/oasdiff-levels.txt" --fail-on ERR --flatten-allof "${ignore[@]}"
