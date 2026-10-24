#!/usr/bin/env bash
# Whether the pinned API contract breaks a client of the newest release tag, through oasdiff
# and `contract-breaks.py`: something removed or renamed, a type changed, an input made
# required, a value refused, or a response value a client may not know.
#
#   scripts/check-api-breaks.sh
#
# Before 1.0 a release may break v1, each break it accepts a line of
# `scripts/api-breaks/since-<tag>.txt` named in its notes. From 1.0 a break belongs in `/api/v2`.
# Needs the tags and their history (`fetch-depth: 0` in CI).
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
routes=$(mktemp)
trap 'rm -f "$base" "$routes"' EXIT
if ! git -C "$ROOT" show "$tag:$CONTRACT" > "$base" 2>/dev/null; then
  echo "$tag has no pinned contract: nothing to compare with."
  exit 0
fi
# The release's route table, whose owner routes its contract leaves out.
git -C "$ROOT" show "$tag:backend/src/main.rs" > "$routes" 2>/dev/null || true

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
# What oasdiff does not read: each operation's scope, the notification payload, a deprecated path
# no release published. Its verdict waits for oasdiff's, so one run reports every break.
extras=0
accepting=()
[ -e "$accepted" ] && accepting=("$accepted")
python3 "$ROOT/scripts/contract-breaks.py" --routes "$routes" "$base" "$ROOT/$CONTRACT" \
  "${accepting[@]}" || extras=1
# oasdiff grades a removed optional field as information, since a client may
# not read it. The contract promises no documented field disappears, so
# `oasdiff-levels.txt` raises that one to an error. A type whose fields another
# one adds to (`#[serde(flatten)]`) is published as `allOf`, and compared
# branch by branch, every field of it reads as removed.
operations=0
oasdiff breaking "$base" "$ROOT/$CONTRACT" \
  --severity-levels "$ROOT/scripts/oasdiff-levels.txt" --fail-on ERR --flatten-allof "${ignore[@]}" ||
  operations=1
[ "$extras" = 0 ] && [ "$operations" = 0 ]
