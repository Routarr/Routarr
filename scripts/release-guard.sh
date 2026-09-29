#!/usr/bin/env bash
# Whether a tagged commit may be published, which release.yml asks before
# anything leaves the runner: GHCR is not a place to take something back from.
#
#   GH_TOKEN=... scripts/release-guard.sh OWNER/REPOSITORY SHA TAG
#
# It only reads, so a dry run from a clone gets the answer the release would.
set -euo pipefail

REPOSITORY=$1
SHA=$2
TAG=$3

refuse() {
  echo "::error::$1"
  exit 1
}

# On main. A tag pushed on a branch head whose pull request ran green would
# otherwise publish `latest` from a tree main never held.
where=$(gh api "repos/$REPOSITORY/compare/main...$SHA" --jq .status)
case "$where" in
  identical | behind) ;;
  *) refuse "$SHA is not on main (it is $where of it). Tag a commit of main." ;;
esac

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

# Not published yet. A tag deleted and pushed again would republish the version
# from another tree, and a backup the first image took could carry a schema the
# second refuses to restore. Publishing it again on purpose means deleting that
# version from the registry first. Any answer but "absent" refuses.
image="${REPOSITORY,,}"
version="${TAG#v}"
token=$(curl -sf "https://ghcr.io/token?scope=repository:$image:pull" \
  | sed -nE 's/.*"token":"([^"]+)".*/\1/p') || refuse "The registry gave no token for $image. Nothing published."
status=$(curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $token" \
  -H "Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json" \
  "https://ghcr.io/v2/$image/manifests/$version")
case "$status" in
  404) ;;
  200) refuse "ghcr.io/$image:$version is already published. Delete that version from the registry to publish it again." ;;
  *) refuse "The registry answered $status for ghcr.io/$image:$version. Nothing published." ;;
esac

echo "$TAG on $SHA may be published."
