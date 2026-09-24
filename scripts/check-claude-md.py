#!/usr/bin/env python3
"""Keep the files Claude Code loads as memory short enough to be followed.

`CLAUDE.md` is read whole at the start of every session, so every line in it
is paid for on every task, and a long file is followed less closely than a
short one. It stays under MAX_LINES, and so does each rule. A line is counted
as it is written, so every line also stays within LINE_WIDTH characters: a
paragraph kept on one long line would pass the count while costing what the
lines it hides cost.

A rule anywhere under `.claude/rules/` loads only when Claude reads a file its
`paths:` list matches. A rule without that list, or with frontmatter that is
not valid YAML, loads in every session like `CLAUDE.md`. An unquoted glob
such as `**/*.ts` is not valid YAML, so each glob is quoted.

Run from anywhere: the paths resolve from this file.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAX_LINES = 200
LINE_WIDTH = 100
PATHS_FRONTMATTER = re.compile(r'\A---\npaths:\n(?:  - "[^"\n]+"\n)+---\n')


def main() -> int:
    problems: list[str] = []

    memory = ROOT / "CLAUDE.md"
    rules = sorted((ROOT / ".claude" / "rules").rglob("*.md"))

    for path in [memory, *rules]:
        name = path.relative_to(ROOT)
        text = path.read_text(encoding="utf-8")
        lines = text.splitlines()
        if len(lines) >= MAX_LINES:
            problems.append(f"{name} has {len(lines)} lines, the ceiling is {MAX_LINES - 1}")
        for number, line in enumerate(lines, 1):
            if len(line) > LINE_WIDTH:
                problems.append(
                    f"{name}:{number} is {len(line)} characters wide, the width is {LINE_WIDTH}"
                )
        if path != memory and not PATHS_FRONTMATTER.match(text):
            problems.append(
                f"{name} does not open with a `paths:` list of quoted globs, "
                "so it loads in every session"
            )

    for problem in problems:
        print(f"check-claude-md: {problem}", file=sys.stderr)
    if problems:
        return 1

    print(
        f"check-claude-md: CLAUDE.md and path-scoped rules: {len(rules)}, "
        f"each under {MAX_LINES} lines and {LINE_WIDTH} characters wide"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
