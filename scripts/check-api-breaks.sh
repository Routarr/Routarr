#!/usr/bin/env bash
# Whether the pinned API contract breaks a client of the last release.
#
#   scripts/check-api-breaks.sh
#
# Compares backend/openapi/v1.json with the same file at the newest `v*` tag,
# through oasdiff: an operation, a parameter or a field removed or renamed, a
# type changed, an input made required, a value no longer accepted. Anything
# added passes. A break belongs in `/api/v2`, never in v1.
#
# It needs the tags and their history (`fetch-depth: 0` in CI). A release that
# predates the pinned contract has nothing to compare with, and passes.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONTRACT=backend/openapi/v1.json

tag=$(git -C "$ROOT" tag --list 'v*' --sort=-version:refname | head -n 1)
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
sys.exit(1 if changed else 0)
PY
# oasdiff grades a removed optional field as information, since a client may
# not read it. The contract promises no documented field disappears, so
# `oasdiff-levels.txt` raises that one to an error.
oasdiff breaking "$base" "$ROOT/$CONTRACT" \
  --severity-levels "$ROOT/scripts/oasdiff-levels.txt" --fail-on ERR
