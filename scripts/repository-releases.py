#!/usr/bin/env python3
"""The releases whose packages the apt and dnf repositories carry, which release.yml asks once a
release is published: the three newest of the newest minor line, so a patch can be undone, and
the newest of each older line, so an installation left on that line still finds its repository.
Only the newest release is supported (SECURITY.md), and GitHub Pages serves 1 GB at most.

    gh api --paginate "repos/OWNER/REPOSITORY/releases?per_page=100" \\
      --jq '.[] | select(.draft or .prerelease | not) | .tag_name' \\
      | python3 scripts/repository-releases.py

Reads the published releases' tags on stdin and prints the tags kept, one per line, oldest first.
"""

from __future__ import annotations

import re
import sys

VERSION = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")

# How many releases of the newest line the repositories keep.
NEWEST_LINE_KEPT = 3


def parsed(tag: str) -> tuple[int, int, int] | None:
    match = VERSION.match(tag.strip())
    return (int(match[1]), int(match[2]), int(match[3])) if match else None


def kept(published: list[str]) -> list[str]:
    """The tags the repositories carry, given the tags of every published release."""
    versions = sorted({v for v in map(parsed, published) if v})
    if not versions:
        return []
    lines: dict[tuple[int, int], list[tuple[int, int, int]]] = {}
    for version in versions:
        lines.setdefault(version[:2], []).append(version)
    newest = max(lines)
    chosen = []
    for line, releases in sorted(lines.items()):
        chosen += releases[-NEWEST_LINE_KEPT:] if line == newest else releases[-1:]
    return [f"v{major}.{minor}.{patch}" for major, minor, patch in chosen]


def main() -> int:
    for tag in kept(sys.stdin.read().split()):
        print(tag)
    return 0


if __name__ == "__main__":
    sys.exit(main())
