#!/usr/bin/env python3
"""The tags people follow that a published release moves onto its image, which release.yml
asks once the release is published: `X.Y` when it is the newest of its minor, `X` from 1.0 on
when it is the newest of its major, and `latest` when it is the newest release of all. A patch
of an older minor therefore leaves `latest` on the newest, and a pre-release moves nothing.

    gh api --paginate "repos/OWNER/REPOSITORY/releases?per_page=100" \\
      --jq '.[] | select(.draft or .prerelease | not) | .tag_name' \\
      | python3 scripts/release-tags.py vX.Y.Z

Reads the published releases' tags on stdin and prints one tag to move per line.
"""

from __future__ import annotations

import re
import sys

VERSION = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")


def parsed(tag: str) -> tuple[int, int, int] | None:
    match = VERSION.match(tag.strip())
    return (int(match[1]), int(match[2]), int(match[3])) if match else None


def moving(tag: str, published: list[str]) -> list[str]:
    """The tags `tag` takes, given the tags of every published release."""
    version = parsed(tag)
    if version is None:
        return []
    # Counted whether or not the list has caught up with the publication.
    versions = {v for v in map(parsed, published) if v} | {version}
    major, minor, _ = version
    tags = []
    if version == max(v for v in versions if v[:2] == (major, minor)):
        tags.append(f"{major}.{minor}")
    # Semantic versioning promises nothing across a 0.x minor, so no `0` tag.
    if major >= 1 and version == max(v for v in versions if v[0] == major):
        tags.append(f"{major}")
    if version == max(versions):
        tags.append("latest")
    return tags


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    for tag in moving(sys.argv[1], sys.stdin.read().split()):
        print(tag)
    return 0


if __name__ == "__main__":
    sys.exit(main())
