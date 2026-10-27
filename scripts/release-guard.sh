#!/usr/bin/env bash
# Whether a tagged commit may be published, which release.yml asks before
# anything leaves the runner: GHCR is not a place to take something back from.
#
#   GH_TOKEN=... scripts/release-guard.sh OWNER/REPOSITORY SHA TAG
#   GH_TOKEN=... scripts/release-guard.sh --publishing OWNER/REPOSITORY SHA TAG
#   GH_TOKEN=... scripts/release-guard.sh --promoting OWNER/REPOSITORY SHA TAG
#
# The second form is asked again just before the manifest names the version,
# since the images take long enough to build for the tag to move meanwhile. The
# third, when the release is published, before `latest` and `X.Y` move onto it.
#
# It only reads, so a dry run from a clone gets the answer the release would.
set -euo pipefail

mode=release
case "${1:-}" in
  --publishing | --promoting)
    mode="${1#--}"
    shift
    ;;
esac
REPOSITORY=$1
SHA=$2
TAG=$3
VERSION="${TAG#v}"

refuse() {
  echo "::error::$1"
  exit 1
}

# Still the tagged commit. A tag deleted and pushed again on another commit
# starts a second release, and the run that finishes last would own the
# version's tags and its attestation.
still_tagged() {
  local tagged
  tagged=$(gh api "repos/$REPOSITORY/commits/tags/$TAG" --jq .sha) ||
    refuse "$TAG no longer exists. Nothing published."
  [ "$tagged" = "$SHA" ] || refuse "$TAG now names $tagged, not $SHA. Nothing published."
}

# Not published yet: a version republished from another tree could refuse the backups the
# first image took. Any answer but 404 refuses.
unpublished() {
  local image="${REPOSITORY,,}" token status
  token=$(curl -sf "https://ghcr.io/token?scope=repository:$image:pull" \
    | sed -nE 's/.*"token":"([^"]+)".*/\1/p') || refuse "The registry gave no token for $image. Nothing published."
  status=$(curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $token" \
    -H "Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json" \
    "https://ghcr.io/v2/$image/manifests/$VERSION")
  case "$status" in
    404) ;;
    200) refuse "ghcr.io/$image:$VERSION is already published. Delete that version from the registry to publish it again." ;;
    *) refuse "The registry answered $status for ghcr.io/$image:$VERSION. Nothing published." ;;
  esac
}

# A later step of the manifest job failing, the attestation say, leaves the
# version published by this run, and re-running the failed job finishes it.
if [ "$mode" = publishing ]; then
  still_tagged
  if [ "${GITHUB_RUN_ATTEMPT:-1}" = 1 ]; then
    unpublished
  fi
  echo "$TAG still names $SHA, and this run may publish $VERSION."
  exit 0
fi
if [ "$mode" = promoting ]; then
  still_tagged
  echo "$TAG still names $SHA, and the tags people follow may move onto $VERSION."
  exit 0
fi

# Annotated, as CONTRIBUTING.md asks: the tag records who made it and when. A
# published release locks its tag, so a lightweight one could never be mended.
kind=$(gh api "repos/$REPOSITORY/git/ref/tags/$TAG" --jq .object.type) ||
  refuse "$TAG cannot be read. Nothing published."
[ "$kind" = tag ] ||
  refuse "$TAG is a lightweight tag. Delete it and push an annotated one: git tag -a $TAG -m \"Routarr $VERSION\" $SHA."

# On main, or on the release branch of its minor for a patch while main holds
# the next minor's breaks. A tag pushed on a branch head whose pull request ran
# green would otherwise publish from a tree neither branch ever held.
on() {
  local where
  where=$(gh api "repos/$REPOSITORY/compare/$1...$SHA" --jq .status 2>/dev/null) || return 1
  [ "$where" = identical ] || [ "$where" = behind ]
}
minor=""
[[ "$VERSION" =~ ^([0-9]+\.[0-9]+)\. ]] && minor="release/${BASH_REMATCH[1]}"
if ! on main && ! { [ -n "$minor" ] && on "$minor"; }; then
  refuse "$SHA is on neither main nor ${minor:-a release branch}. Tag a commit of one of them."
fi

# The version the binary reports, the image's label and the site all read
# `backend/Cargo.toml`. A tag naming another one would publish under that name
# a binary reporting the previous version, and refuse the real one later as
# already published.
crate=$(gh api "repos/$REPOSITORY/contents/backend/Cargo.toml?ref=$SHA" \
  -H 'Accept: application/vnd.github.raw' |
  sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"$/\1/p')
[ -n "$crate" ] || refuse "No version in backend/Cargo.toml at $SHA."
[ "$crate" = "$VERSION" ] ||
  refuse "$TAG names $VERSION, but backend/Cargo.toml at $SHA says $crate. Set the version first, then tag that commit."

# Green in ci.yml and docker.yml on a push of the commit (`scripts/release-runs.py`).
gh api "repos/$REPOSITORY/actions/runs?head_sha=$SHA&per_page=100" |
  python3 "$(dirname "${BASH_SOURCE[0]}")/release-runs.py" ||
  refuse "$SHA is not green in CI and Docker image. Push it to main and let both finish before tagging it."

# One migration file per release, and the shipped ones as they shipped (`check-migrations.py`).
python3 "$(dirname "${BASH_SOURCE[0]}")/check-migrations.py" --release "$VERSION" ||
  refuse "The migrations of $SHA are not ready for $TAG. Fold them into one file first."

unpublished

echo "$TAG on $SHA may be published."
