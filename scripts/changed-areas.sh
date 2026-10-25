#!/usr/bin/env bash
# Which deliverables changed between two commits: the `backend`, `frontend` and `site` lines
# ci.yml's `changes` job hands its jobs, and the `image` and `devcontainer` lines of docker.yml.
#
#   scripts/changed-areas.sh BASE [HEAD]
#
# HEAD `WORKTREE` compares BASE with the working tree, untracked files included
# (scripts/check.sh). A base that cannot be resolved or compared (a force push, a first push, a
# shallow fetch, no green run on main yet) selects everything.
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
  if [ "$HEAD" = WORKTREE ]; then
    listed() { git diff --name-only --no-renames "$BASE" && git ls-files --others --exclude-standard; }
  else
    listed() { git diff --name-only --no-renames "$BASE" "$HEAD"; }
  fi
  if changed=$(listed); then
    printf 'Changed since %s:\n%s\n' "$BASE" "$changed" >&2
  else
    echo "Cannot compare '$BASE' with '$HEAD', running everything." >&2
    everything=true
  fi
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
# both read the frontend's copy of the API types, as a backend test does. A
# backend test holds the dashboard and the alert rules to the metrics.
echo "backend=$(decide "^(backend/|scripts/|frontend/src/api/types\.ts|contrib/monitoring/|$CONTROL)")"
# The frontend job alone runs the bundle size check, and its tests read these
# backend files: the routes the client calls, the settings' defaults, the
# bounds its fields and exports repeat, the count placeholders, the dictionary
# its components render, the warning codes it links to a screen, and the tables
# of cases both sides are held to.
FRONTEND_READS='backend/locales/en\.json|backend/src/(main|state|localization|jobs/mod|api/(deprecated|auth|health|move_log))\.rs|backend/src/services/(metadata|settings|rule_engine/validation|backup/sealed)\.rs|backend/src/.*\.json'
echo "frontend=$(decide "^(frontend/|scripts/check-bundle-size\.mjs|$FRONTEND_READS|$CONTROL)")"
# What the site's checks read from other deliverables: the crate's version, the dictionaries,
# the contract, the README, the first-run screen, and the frontend's Playwright (`verify.mjs`).
echo "site=$(decide "^(site/|backend/(Cargo\.toml|locales/|openapi/v1\.json)|frontend/package(-lock)?\.json|README\.md|frontend/src/components/ApiKeyGate\.svelte|$CONTROL)")"
# What the root Dockerfile builds or copies, what starts the image, and the
# compose file whose container name and data path the first-run command names.
# Not ci.yml: nothing in it reaches the image.
echo "image=$(decide '^(Dockerfile|\.dockerignore|backend/|frontend/|LICENSE|docker-compose\.yml|scripts/(changed-areas|smoke-image|install-runtime)\.sh|\.github/workflows/docker\.yml)')"
# What the dev container's Dockerfile builds from: its own directory, the
# toolchain file it copies, and the ignore file that decides what it can copy.
echo "devcontainer=$(decide '^(\.devcontainer/|backend/rust-toolchain\.toml|\.dockerignore|scripts/changed-areas\.sh|\.github/workflows/docker\.yml)')"
