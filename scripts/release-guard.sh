#!/usr/bin/env bash
# Whether a tagged commit may be published, which release.yml asks before
# anything leaves the runner: GHCR is not a place to take something back from.
#
#   GH_TOKEN=... scripts/release-guard.sh OWNER/REPOSITORY SHA TAG
#   GH_TOKEN=... scripts/release-guard.sh --publishing OWNER/REPOSITORY SHA TAG
#
# The second form is asked again just before the manifest names the version,
# since the images take long enough to build for the tag to move meanwhile.
#
# It only reads, so a dry run from a clone gets the answer the release would.
set -euo pipefail

publishing=false
if [ "${1:-}" = --publishing ]; then
  publishing=true
  shift
fi
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

# Not published yet. A tag deleted and pushed again would republish the version
# from another tree, and a backup the first image took could carry a schema the
# second refuses to restore. Publishing it again on purpose means deleting that
# version from the registry first. Any answer but "absent" refuses.
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
if [ "$publishing" = true ]; then
  still_tagged
  if [ "${GITHUB_RUN_ATTEMPT:-1}" = 1 ]; then
    unpublished
  fi
  echo "$TAG still names $SHA, and this run may publish $VERSION."
  exit 0
fi

# On main. A tag pushed on a branch head whose pull request ran green would
# otherwise publish `latest` from a tree main never held.
where=$(gh api "repos/$REPOSITORY/compare/main...$SHA" --jq .status)
case "$where" in
  identical | behind) ;;
  *) refuse "$SHA is not on main (it is $where of it). Tag a commit of main." ;;
esac

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

runs=$(gh api "repos/$REPOSITORY/actions/runs?head_sha=$SHA&per_page=100" \
  --jq '.workflow_runs[] | select(.name != "Release") | "\(.name)\t\(.conclusion // .status)"')
if [ -z "$runs" ]; then
  refuse "No workflow run for $SHA. Push the commit to main and let CI finish before tagging it."
fi
printf '%s\n' "$runs"

# A skipped run is a legitimate answer: the path filters skip whole areas a
# commit did not touch. A failed or cancelled one is not.
#
# awk, not grep: the two fields are separated by a tab, and `\t` in an ERE is a
# literal `t`, so a grep pattern written that way matches nothing and refuses
# every commit ever tagged.
rejected=$(printf '%s\n' "$runs" | awk -F'\t' '$2 != "success" && $2 != "skipped"')
if [ -n "$rejected" ]; then
  printf 'Not green:\n%s\n' "$rejected"
  refuse "At least one workflow on $SHA did not succeed. Nothing published."
fi

unpublished

echo "$TAG on $SHA may be published."
