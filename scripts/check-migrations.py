#!/usr/bin/env python3
"""Hold the migrations every installation depends on.

A database records the names of the migrations it ran, so a shipped file edited afterwards reaches
no installation that already ran it: its statements stay as the last release shipped them, its
comments aside. With `--release X.Y.Z`, the files added since the last release are at most one,
`NNN_release_X_Y_Z.sql`, since folding them after a pre-release breaks every database it opened.

Run from the repository, or any clone of it. Exits non-zero naming each file at fault.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

MIGRATIONS = "backend/migrations"


def git(*arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments], capture_output=True, text=True, check=True
    ).stdout


def statements(sql: str) -> str:
    """The SQL without its comments or its layout, which an edit may change freely."""
    kept = []
    for line in sql.splitlines():
        quoted = False
        for at, char in enumerate(line):
            if char == "'":
                quoted = not quoted
            elif not quoted and line.startswith("--", at):
                line = line[:at]
                break
        kept.append(line)
    return " ".join(" ".join(kept).split())


def last_release() -> str | None:
    """The newest `v*` tag behind HEAD that is not HEAD itself, which the guard runs on."""
    head = git("rev-parse", "HEAD").strip()
    for tag in git("tag", "--list", "v*", "--merged", "HEAD", "--sort=-v:refname").split():
        if git("rev-list", "-n", "1", tag).strip() != head:
            return tag
    return None


def faults(release: str | None) -> list[str]:
    root = Path(git("rev-parse", "--show-toplevel").strip())
    tag = last_release()
    if tag is None:
        return []
    shipped = git("ls-tree", "--name-only", f"{tag}:{MIGRATIONS}").split()
    found = []
    for name in shipped:
        now = root / MIGRATIONS / name
        if not now.is_file():
            found.append(f"{MIGRATIONS}/{name} shipped in {tag} and is gone")
        elif statements(now.read_text()) != statements(git("show", f"{tag}:{MIGRATIONS}/{name}")):
            found.append(f"{MIGRATIONS}/{name} shipped in {tag}: change the schema in a new file")
    if release is not None:
        added = sorted(
            path.name for path in (root / MIGRATIONS).glob("*.sql") if path.name not in shipped
        )
        version = release.split("-")[0].replace(".", "_")
        if len(added) > 1 or any(not re.fullmatch(rf"\d{{3}}_release_{version}\.sql", name)
                                 for name in added):
            found.append(
                f"the files added since {tag} ({', '.join(added)}) fold into one "
                f"NNN_release_{version}.sql before the tag"
            )
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--release", help="the version a tag publishes")
    found = faults(parser.parse_args().release)
    for fault in found:
        print(f"error: {fault}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
