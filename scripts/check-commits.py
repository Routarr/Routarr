#!/usr/bin/env python3
"""Whether a branch's commits and its pull request's title keep the commit rules of
CONTRIBUTING.md: a type of its list, an optional scope, a lower-case subject of 72 characters
at most and no full stop. The title counts too: it becomes the body of the merge commit.

    python3 scripts/check-commits.py [--title TITLE] [--author LOGIN] [BASE]

BASE defaults to the merge base with origin/main. Merge commits are git's, and Dependabot's
commits and pull requests write `Bump` capitalised, so both are left out. Exits 1 naming each
subject refused.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

TYPES = ("feat", "fix", "docs", "test", "refactor", "perf", "build", "ci", "chore", "revert")
SUBJECT = re.compile(rf"^({'|'.join(TYPES)})(\([a-z0-9-]+\))?!?: [a-z0-9`]")
LIMIT = 72
DEPENDABOT = "dependabot[bot]"


def refusal(subject: str) -> str | None:
    """Why a subject breaks the rules, or None."""
    if len(subject) > LIMIT:
        return f"{len(subject)} characters, {LIMIT} at most"
    if not SUBJECT.match(subject):
        return f"not `type(scope): subject` with a type among {', '.join(TYPES)} and a lower-case start"
    if subject.endswith("."):
        return "ends with a full stop"
    return None


def problems(commits: list[tuple[str, str, str]], title: str, author: str) -> list[str]:
    """Every commit `(sha, author, subject)` and the title that break the rules, as lines to print."""
    found = [
        f"{sha} {subject}\n    {why}"
        for sha, name, subject in commits
        if name != DEPENDABOT and (why := refusal(subject))
    ]
    if title and author != DEPENDABOT and (why := refusal(title)):
        found.append(f"the pull request title {title}\n    {why}")
    return found


def branch_commits(base: str) -> list[tuple[str, str, str]]:
    listed = subprocess.run(
        ["git", "log", "--no-merges", "--format=%h%x00%an%x00%s", f"{base}..HEAD"],
        capture_output=True, text=True, check=True,
    ).stdout
    return [tuple(line.split("\0", 2)) for line in listed.splitlines() if line]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("base", nargs="?", default="", help="the commit the branch starts from")
    parser.add_argument("--title", default="", help="the pull request's title")
    parser.add_argument("--author", default="", help="the login of the pull request's author")
    args = parser.parse_args()

    base = args.base
    if not base:
        found = subprocess.run(["git", "merge-base", "HEAD", "origin/main"],
                               capture_output=True, text=True, check=False)
        base = found.stdout.strip()
    commits = branch_commits(base) if base else []
    if not base:
        print("check-commits: no origin/main to compare with, so no commit is read (pass BASE)",
              file=sys.stderr)

    found = problems(commits, args.title, args.author)
    if found:
        print("check-commits: these break the commit rules of CONTRIBUTING.md\n")
        print("\n".join(found))
        return 1
    print(f"check-commits: {len(commits)} commit(s){' and the title' if args.title else ''} keep the rules")
    return 0


if __name__ == "__main__":
    sys.exit(main())
