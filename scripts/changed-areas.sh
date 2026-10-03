#!/usr/bin/env bash
# Which of the three deliverables changed between two commits: the `backend`,
# `frontend` and `site` lines the `changes` job of ci.yml hands its jobs.
#
#   scripts/changed-areas.sh BASE [HEAD]
#
# The three directories are disjoint, so a site-only commit costs neither the
# Rust jobs nor the end-to-end suite.
#
# It fails open. A base that cannot be resolved (a force push, the first push
# to a branch, a shallow fetch, no successful run on main yet) selects
# everything: skipping too much lets a broken commit through, running too much
# costs minutes.
set -u

BASE="${1:-}"
HEAD="${2:-HEAD}"

everything=false
changed=""
if [ -z "$BASE" ] \
  || [ "$BASE" = "0000000000000000000000000000000000000000" ] \
  || ! git cat-file -e "$BASE^{commit}" 2>/dev/null; then
  echo "No usable base commit ('$BASE'), running everything." >&2
  everything=true
else
  # Without `--no-renames` a moved file is listed by its destination alone, and
  # moving one out of an area wakes nothing that builds that area.
  changed=$(git diff --name-only --no-renames "$BASE" "$HEAD")
  printf 'Changed since %s:\n%s\n' "$BASE" "$changed" >&2
fi

decide() {
  if [ "$everything" = true ]; then
    echo true
  elif printf '%s\n' "$changed" | grep -qE "$1"; then
    echo true
  else
    echo false
  fi
}

# What decides for every area wakes them all.
CONTROL='\.github/workflows/ci\.yml|scripts/changed-areas\.sh'

# `scripts/` holds the locale and API type checks the backend job runs, and
# both read the frontend's copy of the API types, as a backend test does.
echo "backend=$(decide "^(backend/|scripts/|frontend/src/api/types\.ts|$CONTROL)")"
# The frontend job alone runs the bundle size check, and the rule editor's
# folding is held to the engine's table of cases.
echo "frontend=$(decide "^(frontend/|scripts/check-bundle-size\.mjs|backend/src/services/normalise_value_cases\.json|$CONTROL)")"
# `site/check.mjs` reads the version out of the crate's manifest, holds the
# README and the first-run screen to one command and the site's words to the
# application's dictionaries, the API page is built from the pinned contract,
# and `site/verify.mjs` loads Playwright out of the frontend's `node_modules`.
# Each is a real dependency on another deliverable: a Dependabot bump of
# `@playwright/test` matches nothing else the site job watches, and would land
# green while breaking it.
echo "site=$(decide "^(site/|backend/(Cargo\.toml|openapi/v1\.json|locales/)|frontend/package(-lock)?\.json|README\.md|frontend/src/components/ApiKeyGate\.svelte|$CONTROL)")"
