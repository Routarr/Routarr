#!/usr/bin/env python3
"""Guard the translation dictionaries.

1. Every key the code references exists in `locales/en.json`.
2. Placeholders match across languages.
3. No language carries a key English does not have.
4. No key outlives its last use.
5. No language falls below MIN_COMPLETION, a partial translation being welcome.
6. Every dictionary keeps the English key order `add-locale.py` writes.
7. A core term reads one way in each language (`scripts/glossary.json`).
8. No confirmation button starts with the language's Cancel word.
9. Every placeholder is a count (`COUNTS` in `backend/src/localization.rs`) or in `NOT_COUNTS`.
10. French puts a no-break space before `? ! : ; % »` and after `«`.

Run from the repository root: `python3 scripts/check-locales.py`
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOCALES = ROOT / "backend" / "locales"

# Keys built at run time: the prefix joined in PascalCase with each value the pattern
# reads in the file, a capital value as it is. `Prefix+Suffix` ends each key with it.
BUILT = {
    "ConditionLabel": ("backend/src/api/conditions.rs", r'^\s*kind: "([a-z_]+)",$'),
    # The sentence of each condition, a yes or no one said no, and an open range.
    "ConditionPhrase": ("backend/src/api/conditions.rs", r'^\s*kind: "([a-z_]+)",$'),
    "ConditionPhrase+Not": (
        "backend/src/api/conditions.rs",
        r'kind: "([a-z_]+)",\n\s*value_type: "boolean"',
    ),
    "ConditionPhrase+From": (
        "backend/src/api/conditions.rs",
        r'kind: "([a-z_]+)",\n\s*value_type: "(?:year|number|score)_range"',
    ),
    "ConditionPhrase+To": (
        "backend/src/api/conditions.rs",
        r'kind: "([a-z_]+)",\n\s*value_type: "(?:year|number|score)_range"',
    ),
    "Job": ("backend/src/jobs/registry.rs", r'JobKind::\w+ => "([a-z_]+)"'),
    "Trigger": ("backend/src/jobs/mod.rs", r'pub const TRIGGER_\w+: &str = "([a-z_]+)";'),
    # A job's and a decision's, as the frontend types them.
    "Status": (
        "frontend/src/api/types.ts",
        r"(?:DecisionStatus = |^  status: )((?:'[a-z]+'(?: \| )?)+);",
    ),
    # What a security event is about, on the security log screen.
    "SecurityKind": ("backend/src/services/audit.rs", r'Kind::\w+ => "([a-z_]+)"'),
    # The mode that opened a session, on the sessions card.
    "SessionSource": ("frontend/src/api/types.ts", r"^  source: ((?:'[a-z]+'(?: \| )?)+);"),
    "CountryRetired": (
        "backend/src/integrations/language/mod.rs",
        r"pub const RETIRED: &\[&str\] = &\[([^\]]+)\];",
    ),
}

# Below this a language is more English than its own: a broken file.
MIN_COMPLETION = 0.50

GLOSSARY = ROOT / "scripts" / "glossary.json"

COUNTS_SOURCE = ROOT / "backend" / "src" / "localization.rs"

# The placeholders that hold anything but a count: names, paths, codes, years, ids.
NOT_COUNTS = {
    "address", "age", "arr", "base", "category", "cause", "certification", "code", "condition",
    "countries", "country", "detail", "ends", "entry", "error", "event", "expected", "field", "file", "first",
    "folder", "found", "free", "handle", "host", "id", "index", "instance", "issue", "key", "kind", "label",
    "language", "last", "max", "message", "min", "minimum", "name", "names", "needed", "next",
    "number", "observed", "opened", "origin", "path", "paths", "provider", "query", "reason",
    "reasons", "template", "regions", "route", "rule", "scale", "schema", "scope", "scopes", "screen", "second",
    "section", "service", "setting", "since", "size", "source", "sources", "status", "supported",
    "tab", "terms", "title", "titles", "type", "url", "used", "value", "values", "variable", "version", "when", "year",
}


def source_text() -> str:
    parts = []
    for directory in ("backend/src", "frontend/src"):
        for path in (ROOT / directory).rglob("*"):
            # Tests deliberately use made-up keys to exercise the fallback.
            if ".test." in path.name:
                continue
            # And so does their scaffolding, and a key only a test quotes is an orphan.
            if {"test", "tests"} & set(path.relative_to(ROOT).parts[:-1]):
                continue
            if path.suffix in {".rs", ".ts", ".svelte"} and path.is_file():
                parts.append(strip_rust_tests(path.read_text(encoding="utf-8")))
    return "\n".join(parts)


def strip_rust_tests(text: str) -> str:
    """Drop the trailing `#[cfg(test)] mod tests` block and its made-up keys."""
    marker = "#[cfg(test)]\nmod tests {"
    index = text.find(marker)
    return text if index == -1 else text[:index]


def referenced_keys(text: str) -> set[str]:
    """Keys the code asks for by literal name."""
    patterns = [
        r'translate\(\s*"([A-Z][A-Za-z0-9]*)"',          # Rust: localizer.translate("Key")
        r"\bt\(\s*'([A-Z][A-Za-z0-9]*)'",                 # frontend: t('Key')
        # The section comes first, as a literal or as a variable.
        r'ValidationIssue::(?:error|warning)\(\s*[^,()]+,\s*"([A-Z][A-Za-z0-9]*)"',
        r'"(Condition[A-Z][A-Za-z0-9]*)"',                # rule engine outcome keys
        r'\(\s*"([A-Z][A-Za-z0-9]*)",\s*vec!\[',          # Rust: ("Key", vec![params])
        # Keys held in config objects: `key`, every `…Key`, and a route's `hint`.
        r"\b(?:key|[a-z]+Key|hint):\s*'([A-Z][A-Za-z0-9]*)'",
        r"^\s*[a-z_]+:\s*'([A-Z][A-Za-z0-9]*)',\s*$",         # keys held in lookup maps
        r"\?\s*'([A-Z][A-Za-z0-9]*)'\s*:\s*'([A-Z][A-Za-z0-9]*)'",  # t(cond ? 'A' : 'B')
    ]
    found: set[str] = set()
    for pattern in patterns:
        flags = re.DOTALL if "^" not in pattern else re.MULTILINE
        for match in re.findall(pattern, text, flags):
            if isinstance(match, tuple):
                found |= {group for group in match if group}
            else:
                found.add(match)
    return found


def arguments(text: str, opening: int) -> list[str]:
    """The top-level arguments of the call whose `(` is at `opening`."""
    found, depth, start = [], 0, opening + 1
    for index in range(opening, len(text)):
        char = text[index]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
            if depth == 0:
                found.append(text[start:index])
                return found
        elif char == "," and depth == 1:
            found.append(text[start:index])
            start = index + 1
    return found


def confirmation_labels(text: str) -> set[str]:
    """The keys a button beside Cancel reads: the bare literal `askConfirmation`
    and `answering` take, and the `label` of each option `ask` offers."""
    labels: set[str] = set()
    for call in re.finditer(r"\b(askConfirmation|answering|ask)\(", text):
        found = arguments(text, call.end() - 1)
        if call.group(1) == "ask":
            labels |= set(re.findall(r"\blabel:\s*'([A-Z][A-Za-z0-9]*)'", ",".join(found)))
            continue
        for argument in found:
            literal = re.fullmatch(r"\s*'([A-Z][A-Za-z0-9]*)'\s*", argument)
            if literal:
                labels.add(literal.group(1))
    return labels


def built_keys() -> dict[str, set[str]]:
    """Each family of `BUILT` and the keys the code can assemble for it."""
    families: dict[str, set[str]] = {}
    for family, (relative, pattern) in BUILT.items():
        prefix, _, suffix = family.partition("+")
        text = (ROOT / relative).read_text(encoding="utf-8")
        values = set()
        for found in re.findall(pattern, text, re.MULTILINE):
            values |= set(re.findall(r"[a-z_]+|[A-Z]{2,}", found))
        families[family] = {
            prefix
            + (value if value.isupper() else "".join(w.capitalize() for w in value.split("_")))
            + suffix
            for value in values
        }
    return families


def server_counts() -> set[str]:
    text = COUNTS_SOURCE.read_text(encoding="utf-8")
    found = re.search(r"pub const COUNTS: &\[&str\] = &\[([^\]]*)\]", text)
    return set(re.findall(r'"(\w+)"', found.group(1))) if found else set()


def placeholders(template: str) -> set[str]:
    """The `{name}` values a sentence carries. `add-locale.py` reads this one."""
    return set(re.findall(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}", template))


def not_text(dictionaries: dict) -> list[str]:
    """Drop and name every value that is not text. The server reads a
    dictionary holding one as no dictionary at all, falling back to English
    for the whole language, and every comparison here expects text."""
    problems = []
    for language, dictionary in sorted(dictionaries.items()):
        if not isinstance(dictionary, dict):
            problems.append(f"{language}.json is not a JSON object")
            dictionaries[language] = {}
            continue
        for key in [key for key, value in dictionary.items() if not isinstance(value, str)]:
            problems.append(f"{language}.json: '{key}' is not a string")
            del dictionary[key]
    return problems


def glossary_problems(english: dict[str, str], dictionaries: dict[str, dict[str, str]]) -> list[str]:
    """A key whose English says a concept's word, `except` aside, says the
    language's word. A `homonym` translates without it, and the word may not
    stand `beside` the placeholder naming that thing, `beside_skip` aside."""
    problems: list[str] = []
    # A placeholder names a value, not the concept: `{skipped}` is a count.
    said = {key: re.sub(r"\{[a-zA-Z_][a-zA-Z0-9_]*\}", "", value) for key, value in english.items()}
    for concept, spec in json.loads(GLOSSARY.read_text(encoding="utf-8")).items():
        carriers = [
            key
            for key in english
            if re.search(spec["en"], said[key], re.IGNORECASE) and key not in spec["except"]
        ]
        if not carriers:
            problems.append(f"glossary: '{concept}' matches no English key")
        for key in spec["except"]:
            if key not in english or not re.search(spec["en"], said[key], re.IGNORECASE):
                problems.append(f"glossary: '{concept}' excepts {key}, whose English does not say it")
        for language, dictionary in sorted(dictionaries.items()):
            if language == "en":
                continue
            word = spec["words"].get(language)
            if word is None:
                problems.append(f"glossary: '{concept}' has no word for {language}")
                continue
            for key in carriers:
                value = dictionary.get(key)
                if value is None or value == english[key]:
                    continue
                if not re.search(word, value, re.IGNORECASE):
                    problems.append(
                        f"{language}.json: '{key}' says '{concept}' in another word than /{word}/"
                    )
            homonym = spec.get("homonym")
            if homonym is None:
                continue
            for key, value in english.items():
                translated = dictionary.get(key)
                if key in carriers or translated is None or translated == value:
                    continue
                if re.search(homonym, value) and re.search(word, translated, re.IGNORECASE):
                    problems.append(
                        f"{language}.json: '{key}' uses the '{concept}' word /{word}/ "
                        f"for what its English says as /{homonym}/"
                    )
            beside = spec.get("beside")
            if beside is None or language in spec.get("beside_skip", []):
                continue
            # The word, then at most a space and the placeholder, or the other way.
            adjacent = rf"(?:{word})\w*\s*(?:{beside})|(?:{beside})\s*\S*?(?:{word})"
            for key, value in english.items():
                translated = dictionary.get(key)
                if key in carriers or translated is None or translated == value:
                    continue
                if re.search(beside, value) and re.search(adjacent, translated, re.IGNORECASE):
                    problems.append(
                        f"{language}.json: '{key}' names {re.search(beside, translated).group(0)} "
                        f"with the '{concept}' word /{word}/"
                    )
    return problems


def main() -> int:
    dictionaries = {
        path.stem: json.loads(path.read_text(encoding="utf-8"))
        for path in sorted(LOCALES.glob("*.json"))
    }
    if "en" not in dictionaries:
        print("locales/en.json is missing", file=sys.stderr)
        return 1

    problems: list[str] = not_text(dictionaries)
    english = dictionaries["en"]
    text = source_text()
    referenced = referenced_keys(text)
    families = built_keys()
    built = set().union(*families.values())

    # A pattern that stops matching would exempt nothing and check nothing.
    for prefix, keys in sorted(families.items()):
        if not keys:
            problems.append(f"read no {prefix} value from {BUILT[prefix][0]}")
        for key in sorted(keys - set(english)):
            problems.append(f"built at run time but missing from en.json: {key}")

    # 1. Referenced but not defined, among the identifiers that look like keys.
    for key in sorted(referenced - set(english)):
        if key in {"POST", "PUT", "GET", "DELETE"} or len(key) < 3:
            continue
        if f'"{key}"' in text or f"'{key}'" in text:
            problems.append(f"referenced but missing from en.json: {key}")

    # 2, 3 & 5. Placeholders, stray keys and completion.
    completion = {}
    for language, dictionary in sorted(dictionaries.items()):
        if language == "en":
            continue

        missing = sorted(set(english) - set(dictionary))
        completion[language] = 1 - len(missing) / len(english)

        for key in sorted(set(dictionary) - set(english)):
            problems.append(f"{language}.json has a key English does not: {key}")

        for key, template in english.items():
            translated = dictionary.get(key)
            if translated is None:
                continue
            if placeholders(template) != placeholders(translated):
                problems.append(
                    f"{language}.json: placeholders differ for '{key}' "
                    f"({sorted(placeholders(template))} vs {sorted(placeholders(translated))})"
                )
            if not translated.strip():
                problems.append(f"{language}.json: '{key}' is empty")

        if completion[language] < MIN_COMPLETION:
            problems.append(
                f"{language}.json is only {completion[language]:.0%} translated "
                f"({len(missing)} keys missing, minimum {MIN_COMPLETION:.0%}). "
                f"A partial translation is fine, but this looks like a broken file."
            )

    # 4. Defined but never used.
    for key in sorted(english):
        if key in built:
            continue
        if f'"{key}"' not in text and f"'{key}'" not in text:
            problems.append(f"defined but never referenced: {key}")

    # 6. In the English order, the first key out of place named.
    for language, dictionary in sorted(dictionaries.items()):
        found = [key for key in dictionary if key in english]
        expected = [key for key in english if key in dictionary]
        if found != expected:
            at = next(i for i, (a, b) in enumerate(zip(found, expected)) if a != b)
            problems.append(
                f"{language}.json leaves the English key order at '{found[at]}' "
                f"(expected '{expected[at]}'): "
                f"echo '{{}}' | python3 scripts/add-locale.py {language}"
            )

    # 7. One word per core term.
    problems.extend(glossary_problems(english, dictionaries))

    # 8. A confirmation's button against its Cancel, an untranslated label read as English.
    labels = sorted(confirmation_labels(text))
    for language, dictionary in sorted(dictionaries.items()):
        cancel = dictionary.get("Cancel", english["Cancel"]).split()[0].casefold()
        for key in labels:
            label = dictionary.get(key, english.get(key, ""))
            if label.casefold().startswith(cancel):
                problems.append(
                    f"{language}.json: '{key}' ({label!r}) starts with the Cancel word "
                    f"'{cancel}', and the two sit side by side in a confirmation"
                )

    # 9. Each placeholder a count or not, and each listed name in use.
    counts = server_counts()
    if not counts:
        problems.append(f"read no COUNTS from {COUNTS_SOURCE.relative_to(ROOT)}")
    used = set().union(*(placeholders(template) for template in english.values()))
    for name in sorted(used - counts - NOT_COUNTS):
        problems.append(
            f"placeholder {{{name}}} is neither in COUNTS ({COUNTS_SOURCE.relative_to(ROOT)}) "
            "nor in NOT_COUNTS (scripts/check-locales.py)"
        )
    for name in sorted(counts & NOT_COUNTS):
        problems.append(f"placeholder {{{name}}} is both a count and not one")
    for name in sorted((counts | NOT_COUNTS) - used):
        problems.append(f"placeholder {{{name}}} is classed but no sentence uses it")

    # 10. French spacing.
    for key, value in dictionaries.get("fr", {}).items():
        stranded = re.search(r" [?!:;%»]|« ", value)
        if stranded:
            problems.append(
                f"fr.json: '{key}' has a plain space in {stranded.group(0)!r}, "
                "where French puts a no-break space (U+00A0)"
            )

    if problems:
        print(f"{len(problems)} locale problem(s):", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    print(f"locales OK: {len(english)} keys, {len(dictionaries) - 1} translation(s)")
    for language, ratio in sorted(completion.items(), key=lambda kv: (-kv[1], kv[0])):
        bar = "█" * round(ratio * 20)
        print(f"  {language:6s} {ratio:6.1%} {bar}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
