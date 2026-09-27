#!/usr/bin/env python3
"""Write a locale file from a mapping supplied on stdin, validating as it goes.

    python3 scripts/add-locale.py de "Deutsch" < translations.json

Refuses to write when a translation drops or invents a `{placeholder}`, carries a
key English does not have, or is empty — the three ways a locale silently
degrades the interface.

The Radarr and Sonarr menu paths a string cites ("Settings → General → Security
→ API Key", "System → Logs") stay in English in every language: they are exact
for an Arr in its default language, and the Arrs' own translations change with
their version.
"""
from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOCALES = ROOT / "backend" / "locales"


def placeholders(text: str) -> set[str]:
    return set(re.findall(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}", text))


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: add-locale.py <code> [name] < translations.json", file=sys.stderr)
        return 2

    code = sys.argv[1]
    # The file name and the setting value: a typo here ships a dictionary the
    # picker lists and nothing can select.
    if not re.fullmatch(r"[a-z]{2,3}(_[A-Z]{2}|_[A-Z][a-z]{3})?", code):
        print(f"'{code}' is not a language code of the shape xx or xx_YY", file=sys.stderr)
        return 2
    english = json.loads((LOCALES / "en.json").read_text(encoding="utf-8"))
    incoming = json.loads(sys.stdin.read())

    problems = []
    for key, value in incoming.items():
        if key not in english:
            problems.append(f"unknown key: {key}")
            continue
        if not str(value).strip():
            problems.append(f"empty translation: {key}")
        if placeholders(english[key]) != placeholders(str(value)):
            problems.append(
                f"placeholders differ for '{key}': "
                f"{sorted(placeholders(english[key]))} vs {sorted(placeholders(str(value)))}"
            )

    if problems:
        print(f"{len(problems)} problem(s), nothing written:", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    # Merge over anything already translated, so a batch can extend a language.
    target = LOCALES / f"{code}.json"
    existing = json.loads(target.read_text(encoding="utf-8")) if target.exists() else {}
    merged = {**existing, **incoming}
    # In the English file's order, which every dictionary follows: written
    # sorted instead, a batch of ten keys would move every line of the file.
    merged = {k: merged[k] for k in english if k in merged}

    target.write_text(
        json.dumps(merged, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    ratio = len(merged) / len(english)
    missing = len(english) - len(merged)
    print(f"{code}: {len(merged)}/{len(english)} keys ({ratio:.0%})"
          + (f", {missing} missing" if missing else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
