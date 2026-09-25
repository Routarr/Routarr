#!/usr/bin/env python3
"""Hold the text a reader sees to plain punctuation.

No em dash, no semicolon in running text and no curly quotes: in the
interface's dictionaries, the showcase site's catalogues, the README, and the
strings the frontend and the backend write themselves. The en dash, the
ellipsis, arrows, check marks, the middle dot, angle quotes and corner
brackets are all fine.

Two exceptions belong to the text itself. Greek writes its question mark as
`;`, so that dictionary is read for the Arabic and full-width semicolons alone,
and for its own semicolon, the raised dot, which a middle dot passes for: in
Greek a middle dot is accepted only as a spaced separator. Catalan writes a
middle dot inside a word (col·lecció), so no other language is read for it.
In source code a semicolon is syntax (SQL, a cookie header, CSS), so code is
read for the em dash and curly quotes only. Comments are not read.

Run from anywhere: the paths resolve from this file.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EM_DASHES = "—⸺"
CURLY_QUOTES = "“”‘’„‚‟‛"
SEMICOLONS = ";؛；"
GREEK_QUESTION_MARK = ";"
GREEK_SEMICOLON = re.compile(r"\u0387|(?<! )\u00b7|\u00b7(?! )")
FENCE = re.compile(r"^```.*?^```", re.M | re.S)


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


def readme(problems: list[str]) -> None:
    path = ROOT / "README.md"
    text = path.read_text(encoding="utf-8")
    prose = FENCE.sub(lambda m: "\n" * m.group(0).count("\n"), text)
    for number, (line, prose_line) in enumerate(zip(text.split("\n"), prose.split("\n")), 1):
        names = found(line, "") + (["a semicolon"] if ";" in prose_line else [])
        for name in names:
            problems.append(f"README.md:{number} carries {name}")


def blank(match: re.Match) -> str:
    return re.sub(r"[^\n]", " ", match.group(0))


def without_comments(source: str, markup: bool) -> str:
    if markup:
        source = re.sub(r"<!--.*?-->", blank, source, flags=re.S)
    source = re.sub(r"/\*.*?\*/", blank, source, flags=re.S)
    return re.sub(r"(?<![:'\"`\\])//[^\n]*", blank, source)


def frontend(problems: list[str]) -> int:
    files = sorted(
        path
        for pattern in ("src/**/*.svelte", "src/**/*.ts", "e2e/**/*.ts")
        for path in (ROOT / "frontend").glob(pattern)
    )
    for path in files:
        code = without_comments(path.read_text(encoding="utf-8"), path.suffix == ".svelte")
        for number, line in enumerate(code.split("\n"), 1):
            for name in found(line, ""):
                problems.append(f"{path.relative_to(ROOT)}:{number} carries {name}")
    return len(files)


def rust_literals(source: str):
    """Yield (line, text) for every string literal outside a comment."""
    i, line, size = 0, 1, len(source)
    while i < size:
        if source.startswith("//", i):
            end = source.find("\n", i)
            i = size if end < 0 else end
            continue
        if source.startswith("/*", i):
            end = source.find("*/", i + 2)
            line += source.count("\n", i, end)
            i = end + 2
            continue
        raw = re.match(r'b?r(#*)"', source[i:])
        if raw and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_")):
            close = '"' + raw.group(1)
            end = source.find(close, i + raw.end())
            yield line, source[i + raw.end() : end]
            line += source.count("\n", i, end)
            i = end + len(close)
            continue
        if source[i] == "'":
            char = re.match(r"'(\\.|[^\\'])'", source[i:])
            if char:
                i += char.end()
                continue
        if source[i] == '"':
            end, start = i + 1, line
            while source[end] != '"':
                if source[end] == "\\":
                    end += 1
                if source[end] == "\n":
                    line += 1
                end += 1
            yield start, source[i + 1 : end]
            i = end + 1
            continue
        if source[i] == "\n":
            line += 1
        i += 1


def backend(problems: list[str]) -> int:
    files = sorted((ROOT / "backend" / "src").glob("**/*.rs"))
    for path in files:
        for number, literal in rust_literals(path.read_text(encoding="utf-8")):
            for name in found(literal, ""):
                problems.append(f"{path.relative_to(ROOT)}:{number} carries {name}")
    return len(files)


def main() -> int:
    problems: list[str] = []
    counts = {
        "dictionaries": catalogues(ROOT / "backend" / "locales", problems),
        "site catalogues": catalogues(ROOT / "site" / "src" / "i18n", problems),
        "frontend files": frontend(problems),
        "backend files": backend(problems),
    }
    readme(problems)
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
    print(f"check-typography: {summary} and the README use plain punctuation")
    return 0


if __name__ == "__main__":
    sys.exit(main())
