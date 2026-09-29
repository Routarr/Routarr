#!/usr/bin/env python3
"""Hold the text a reader sees to plain punctuation.

No em dash, no semicolon in running text and no curly quotes: in the
interface's dictionaries, the showcase site's catalogues, the Markdown
documents a contributor reads, and every source file, comments and messages
included. The en dash, the ellipsis, arrows, check marks, the middle dot,
angle quotes and corner brackets are all fine.

Two exceptions belong to the text itself. Greek writes its question mark as
`;`, so that dictionary is read for the Arabic and full-width semicolons alone,
and for its own semicolon, the raised dot, which a middle dot passes for: in
Greek a middle dot is accepted only as a spaced separator. Catalan writes a
middle dot inside a word (col·lecció), so no other language is read for it.
In a source file a semicolon is syntax (Rust, TypeScript, SQL, CSS, a cookie
header), so a source file is read for the em dash and curly quotes only, and
the semicolons of its comments are left to review.

Run from anywhere: the paths resolve from this file.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Written as escapes, since this file is read too.
EM_DASHES = "\u2014\u2e3a"
CURLY_QUOTES = "\u201c\u201d\u2018\u2019\u201e\u201a\u201f\u201b"
SEMICOLONS = ";\u061b\uff1b"
GREEK_QUESTION_MARK = ";"
GREEK_SEMICOLON = re.compile(r"\u0387|(?<! )\u00b7|\u00b7(?! )")
FENCE = re.compile(r"^```.*?^```", re.M | re.S)
MARKDOWN = ("README.md", "SECURITY.md", "CONTRIBUTING.md")


def found(text: str, semicolons: str) -> list[str]:
    names = []
    if any(ch in text for ch in EM_DASHES):
        names.append("an em dash")
    if any(ch in text for ch in CURLY_QUOTES):
        names.append("a curly quote")
    if any(ch in text for ch in semicolons):
        names.append("a semicolon")
    return names


def catalogues(folder: Path, problems: list[str]) -> int:
    files = sorted(folder.glob("*.json"))
    for path in files:
        semicolons = SEMICOLONS
        if path.stem == "el":
            semicolons = SEMICOLONS.replace(GREEK_QUESTION_MARK, "")
        for key, value in json.loads(path.read_text(encoding="utf-8")).items():
            if not isinstance(value, str):
                continue
            names = found(value, semicolons)
            if path.stem == "el" and GREEK_SEMICOLON.search(value):
                names.append("a Greek semicolon")
            for name in names:
                problems.append(f"{path.relative_to(ROOT)}: {key} carries {name}")
    return len(files)


def markdown(problems: list[str]) -> int:
    """Prose outside a fenced block is read for semicolons too."""
    for name in MARKDOWN:
        text = (ROOT / name).read_text(encoding="utf-8")
        prose = FENCE.sub(lambda m: "\n" * m.group(0).count("\n"), text)
        for number, (line, prose_line) in enumerate(zip(text.split("\n"), prose.split("\n")), 1):
            names = found(line, "") + (["a semicolon"] if ";" in prose_line else [])
            for problem in names:
                problems.append(f"{name}:{number} carries {problem}")
    return len(MARKDOWN)


def sources(folder: str, patterns: tuple[str, ...], problems: list[str]) -> int:
    base = ROOT / folder
    files = sorted({path for pattern in patterns for path in base.glob(pattern) if path.is_file()})
    for path in files:
        for number, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
            for name in found(line, ""):
                problems.append(f"{path.relative_to(ROOT)}:{number} carries {name}")
    return len(files)


def main() -> int:
    problems: list[str] = []
    counts = {
        "dictionaries": catalogues(ROOT / "backend" / "locales", problems),
        "site catalogues": catalogues(ROOT / "site" / "src" / "i18n", problems),
        "documents": markdown(problems),
        "backend files": sources(
            "backend",
            ("src/**/*.rs", "migrations/*.sql", "Cargo.toml", "rust-toolchain.toml", ".env.example"),
            problems,
        ),
        # `index.html` holds the title a tab shows before the shell names the
        # screen.
        "frontend files": sources(
            "frontend",
            ("index.html", "*.config.ts", "src/**/*.svelte", "src/**/*.ts", "src/**/*.css",
             "e2e/*.ts", "e2e/*.py", "e2e/*.sh"),
            problems,
        ),
        "site files": sources(
            "site",
            ("*.mjs", "wrangler.jsonc", "public/_headers", "public/assets/*.js", "screenshots/*.*",
             "src/**/*.astro", "src/**/*.ts", "src/**/*.css"),
            problems,
        ),
        "repository files": sources(
            ".",
            ("scripts/*.*", ".github/**/*.yml", ".devcontainer/*.*", ".devcontainer/Dockerfile",
             "Dockerfile", "docker-compose.yml"),
            problems,
        ),
    }
    # A pattern that matches nothing would pass having read nothing.
    empty = [name for name, count in counts.items() if count == 0]
    if empty:
        problems.append(f"read no {', no '.join(empty)}: a path no longer matches")
    for problem in problems:
        print(problem)
    if problems:
        print(f"check-typography: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    summary = ", ".join(f"{count} {name}" for name, count in counts.items())
    print(f"check-typography: {summary} use plain punctuation")
    return 0


if __name__ == "__main__":
    sys.exit(main())
