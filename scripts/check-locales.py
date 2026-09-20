#!/usr/bin/env python3
"""Guard the translation dictionaries.

Checks, in order of how much a failure would hurt:

1. every key referenced in the code exists in `locales/en.json`
   (a missing key renders as a raw identifier in the UI);
2. placeholders match across languages, so a translation cannot drop the very
   value the sentence is about;
3. no language carries a key English does not have (a rename left behind);
4. no key is left behind once its last use is deleted;
5. no language falls below MIN_COMPLETION.

A partial translation is allowed on purpose: an untranslated key falls back to
English at runtime and `GET /localization/languages` reports each language's
completion, so the picker states the truth rather than hiding it. Gating at 90%
would have blocked every translation on its way in, which is the opposite of
what a translation workflow needs. What the floor still catches is a file that
is broken rather than merely incomplete.

Run from the repository root: `python3 scripts/check-locales.py`
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOCALES = ROOT / "backend" / "locales"

# Families whose keys are assembled at runtime (`ConditionLabel` + the condition
# kind, `Job` + the job kind, and so on). Listing the prefixes is enough: the
# per-language parity check still covers the members.
DYNAMIC_PREFIXES = ("ConditionLabel", "Job", "Trigger", "Status")

# Below this a language is more English than its own, which is a broken file
# rather than work in progress. Partial translations above it are welcome: the
# picker shows their completion, so nothing is claimed that is not true.
MIN_COMPLETION = 0.50


def source_text() -> str:
    parts = []
    for directory in ("backend/src", "frontend/src"):
        for path in (ROOT / directory).rglob("*"):
            # Tests deliberately use made-up keys to exercise the fallback.
            if ".test." in path.name:
                continue
            # And so does their scaffolding. `frontend/src/test/` holds the
            # render helper and the payload fixtures, whose plausible-looking
            # values ("Radarr", "Akira") match the lookup-map pattern below and
            # were reported as keys English had lost.
            if "test" in path.relative_to(ROOT).parts[:-1]:
                continue
            if path.suffix in {".rs", ".ts", ".svelte"} and path.is_file():
                parts.append(strip_rust_tests(path.read_text(encoding="utf-8")))
    return "\n".join(parts)


def strip_rust_tests(text: str) -> str:
    """Drop the trailing `#[cfg(test)] mod tests` block.

    Rust test modules live in the same file as the code and use deliberately
    made-up keys to exercise the fallback path.
    """
    marker = "#[cfg(test)]\nmod tests {"
    index = text.find(marker)
    return text if index == -1 else text[:index]


def referenced_keys(text: str) -> set[str]:
    """Keys the code asks for by literal name."""
    patterns = [
        r'translate\(\s*"([A-Z][A-Za-z0-9]*)"',          # Rust: localizer.translate("Key")
        r"\bt\(\s*'([A-Z][A-Za-z0-9]*)'",                 # frontend: t('Key')
        r"translateStatic\(\s*'([A-Z][A-Za-z0-9]*)'",     # frontend, outside a component
        r'ValidationIssue::(?:error|warning)\(\s*"[a-z_]+",\s*"([A-Z][A-Za-z0-9]*)"',
        r'"(Condition[A-Z][A-Za-z0-9]*)"',                # rule engine outcome keys
        r"(?:key|labelKey|helpKey):\s*'([A-Z][A-Za-z0-9]*)'",  # keys held in config objects
        r"^\s*[a-z_]+:\s*'([A-Z][A-Za-z0-9]*)',\s*$",         # keys held in lookup maps
        r"\?\s*'([A-Z][A-Za-z0-9]*)'\s*:\s*'([A-Z][A-Za-z0-9]*)'",  # t(cond ? 'A' : 'B')
    ]
    found: set[str] = set()
    for pattern in patterns:
        flags = re.S if "^" not in pattern else re.M
        for match in re.findall(pattern, text, flags):
            if isinstance(match, tuple):
                found |= {group for group in match if group}
            else:
                found.add(match)
    return found


def placeholders(template: str) -> set[str]:
    return set(re.findall(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}", template))


def main() -> int:
    dictionaries = {
        path.stem: json.loads(path.read_text(encoding="utf-8"))
        for path in sorted(LOCALES.glob("*.json"))
    }
    if "en" not in dictionaries:
        print("locales/en.json is missing", file=sys.stderr)
        return 1

    english = dictionaries["en"]
    text = source_text()
    referenced = referenced_keys(text)
    problems: list[str] = []

    # 1. Referenced but not defined. Only flag identifiers that look like keys we
    #    own, to avoid tripping over unrelated capitalised string literals.
    for key in sorted(referenced - set(english)):
        if key in {"POST", "PUT", "GET", "DELETE"} or len(key) < 3:
            continue
        if any(key.startswith(prefix) for prefix in DYNAMIC_PREFIXES):
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
                f"A partial translation is fine — this looks like a broken file."
            )

    # 4. Defined but never used.
    for key in sorted(english):
        if any(key.startswith(prefix) for prefix in DYNAMIC_PREFIXES):
            continue
        if f'"{key}"' not in text and f"'{key}'" not in text:
            problems.append(f"defined but never referenced: {key}")

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
