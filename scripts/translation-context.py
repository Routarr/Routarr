#!/usr/bin/env python3
"""Write what a translator needs for one batch of keys, one language at a time.

    python3 scripts/translation-context.py batch.json fr de > brief.md
    python3 scripts/translation-context.py batch.json fr --notes notes.md

`batch.json` maps the keys to translate to their English. For each language
the brief holds the batch, the words that language already uses for the
batch's terms (one short existing translation per term and per pair of
terms), the glossary concepts the batch says, the file's punctuation habits,
and the rules `check-locales.py` holds a dictionary to.

A translator reads this instead of the whole dictionary: a tenth of the text
for the same terms, so the words a file already uses are kept. The
translations go back through `add-locale.py`.
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from collections import defaultdict

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOCALES = ROOT / "backend" / "locales"
GLOSSARY = ROOT / "scripts" / "glossary.json"

# An example longer than this costs more to read than it teaches.
LONGEST_EXAMPLE = 90

STOP = set(
    """a about after all also an and any are as at be been before being by can could did do
    does each every for from had has have here how if in into is it its just like may more
    most no not now of off on once one only or other our out over same some still such than
    that the their them then there these they this those to under up was we were what when
    where which while who will with would you your""".split()
)


def words(text: str) -> list[str]:
    """The words of an English string that carry meaning, placeholders left out."""
    text = re.sub(r"\{[a-zA-Z_][a-zA-Z0-9_]*\}", " ", text)
    return [w for w in re.findall(r"[a-z][a-z'-]{2,}", text.lower()) if w not in STOP]


def terms(text: str) -> set[str]:
    """Each meaningful word, and each pair of them side by side: "root folder"
    and "master key" are terms a language may say in a word of its own."""
    found = words(text)
    return set(found) | {f"{a} {b}" for a, b in zip(found, found[1:])}


def examples(batch: dict[str, str], english: dict[str, str], translated: dict[str, str]):
    """One short existing translation for each term of the batch, the shortest."""
    index: dict[str, list[str]] = defaultdict(list)
    for key, value in english.items():
        if key in batch or translated.get(key, value) == value or len(value) > LONGEST_EXAMPLE:
            continue
        for term in terms(value):
            index[term].append(key)
    wanted = set().union(*(terms(value) for value in batch.values())) if batch else set()
    picked: dict[str, str] = {}
    for term in sorted(wanted):
        holders = sorted(index.get(term, []), key=lambda key: (len(english[key]), key))
        if holders:
            picked.setdefault(holders[0], term)
    return sorted(picked, key=lambda key: english[key].lower())


def glossary(batch: dict[str, str], english: dict[str, str], translated: dict[str, str], code: str):
    """The glossary concepts the batch says, each with a translation that uses
    the word the check requires."""
    said = {key: re.sub(r"\{[a-zA-Z_][a-zA-Z0-9_]*\}", "", value) for key, value in english.items()}
    lines = []
    for concept, spec in json.loads(GLOSSARY.read_text(encoding="utf-8")).items():
        if not any(re.search(spec["en"], said[key], re.I) for key in batch):
            continue
        word = spec["words"].get(code)
        carriers = [
            key
            for key in english
            if key not in batch
            and key not in spec["except"]
            and re.search(spec["en"], said[key], re.I)
            and translated.get(key, english[key]) != english[key]
            and word
            and re.search(word, translated[key], re.I)
        ]
        shown = min(carriers, key=lambda key: len(english[key]), default=None)
        if shown:
            lines.append(f"- **{concept}**: {english[shown]} → {translated[shown]}")
        else:
            lines.append(f"- **{concept}**: the word matching `{word}`")
    return lines


def habits(translated: dict[str, str]) -> list[str]:
    """How the file writes quotes and spaces, which a translation has to follow."""
    text = "\n".join(translated.values())
    notes = []
    if "«" in text:
        spaced = " " in text and re.search("« ", text)
        notes.append(
            "Quotes are « », with a no-break space inside each." if spaced else "Quotes are « »."
        )
    elif "「" in text:
        notes.append("Quotes are 「 」.")
    elif '"' in text:
        notes.append('Quotes are straight " ".')
    if re.search(" [:?!]", text):
        notes.append("A no-break space (U+00A0) goes before : ? and !.")
    return notes


RULES = """\
- Keep every `{placeholder}` exactly as written. `add-locale.py` refuses a lost or
  invented one.
- No plural forms: a count is inserted as is. Where the language inflects a noun by
  its number, write a form that reads right for any number ("Events: {count}").
- No em dash, no semicolon (the Greek question mark aside), no curly quotes
  (\u201c \u201d \u2018 \u2019 \u201e \u201a). Use the straight apostrophe.
- A word the examples above use for a term is the word to use. A term they do not
  show gets one word, used in every key of the batch.
- Button and badge labels stay as short as the English. Plain, natural sentences.
- Merge with `python3 scripts/add-locale.py <code> < batch.json`, then run
  `python3 scripts/check-locales.py` and `python3 scripts/check-typography.py`.
"""


def brief(code: str, batch: dict[str, str], english: dict[str, str], notes: str | None) -> str:
    path = LOCALES / f"{code}.json"
    if not path.is_file():
        raise SystemExit(f"no dictionary for '{code}' in {LOCALES}")
    translated = json.loads(path.read_text(encoding="utf-8"))
    parts = [f"# {code}: {len(batch)} keys to translate", ""]
    if notes:
        parts += [notes.strip(), ""]
    parts += ["## The batch", "", "```json", json.dumps(batch, ensure_ascii=False, indent=2), "```", ""]
    shown = examples(batch, english, translated)
    if shown:
        parts += [f"## The words {code}.json already uses", ""]
        parts += [f"- {english[key]} → {translated[key]}" for key in shown]
        parts.append("")
    concepts = glossary(batch, english, translated, code)
    if concepts:
        parts += ["## Glossary terms the batch says (the check requires them)", ""]
        parts += concepts
        parts.append("")
    parts += ["## Rules", ""]
    parts += [f"- {note}" for note in habits(translated)]
    parts.append(RULES)
    return "\n".join(parts)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("batch", type=pathlib.Path, help="the keys to translate, with their English")
    parser.add_argument("codes", nargs="+", help="the languages to write a brief for")
    parser.add_argument("--notes", type=pathlib.Path, help="what the strings are about, for every language")
    arguments = parser.parse_args()

    english = json.loads((LOCALES / "en.json").read_text(encoding="utf-8"))
    batch = json.loads(arguments.batch.read_text(encoding="utf-8"))
    unknown = [key for key in batch if key not in english]
    if unknown:
        print(f"keys en.json does not have: {', '.join(unknown)}", file=sys.stderr)
        return 2
    notes = arguments.notes.read_text(encoding="utf-8") if arguments.notes else None
    print("\n\n".join(brief(code, batch, english, notes) for code in arguments.codes))
    return 0


if __name__ == "__main__":
    sys.exit(main())
