#!/usr/bin/env python3
"""Hold everything the image ships to the one licence allow-list.

`backend/deny.toml` names the licences GPL-3.0-only code may ship beside, and
`cargo deny check` holds the crates to it. This holds the rest:

- every npm package the interface bundles, the entries of
  `frontend/package-lock.json` not marked `dev`, whose licence is read as an
  SPDX expression (`MIT OR Apache-2.0` needs one of the two allowed);
- the `accepted` list of `backend/about.toml`, which writes the crates'
  notices into the image and has to accept exactly what deny.toml allows;
- with `--bundle`, the notices Vite writes for what the build bundled: a package
  named there and marked `dev` would escape this gate and `npm audit --omit=dev`.

Run from the repository root: `python3 scripts/check-licences.py [--bundle FILE]`.
"""

import argparse
import json
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent


def allows(expression: str, allowed: set[str]) -> bool:
    """Whether an SPDX expression is satisfied by the allowed licences.

    `WITH` binds tightest and is matched whole (`Apache-2.0 WITH
    LLVM-exception` is its own entry), then `AND`, then `OR`.
    """
    tokens = re.findall(r"\(|\)|[^\s()]+", expression)
    position = 0

    def peek():
        return tokens[position] if position < len(tokens) else None

    def take():
        nonlocal position
        token = peek()
        position += 1
        return token

    def term() -> bool:
        if peek() == "(":
            take()
            value = either()
            if take() != ")":
                raise ValueError(f"unbalanced parentheses in {expression!r}")
            return value
        licence = take()
        if licence is None or licence in ("AND", "OR", "WITH", ")"):
            raise ValueError(f"no licence where one belongs in {expression!r}")
        if peek() == "WITH":
            take()
            licence = f"{licence} WITH {take()}"
        return licence in allowed

    def both() -> bool:
        value = term()
        while peek() == "AND":
            take()
            value = term() and value
        return value

    def either() -> bool:
        value = both()
        while peek() == "OR":
            take()
            value = both() or value
        return value

    value = either()
    if peek() is not None:
        raise ValueError(f"unexpected {peek()!r} in {expression!r}")
    return value


def npm_refusals(lock: dict, allowed: set[str]) -> list[str]:
    """The bundled packages whose licence the allow-list does not cover."""
    refusals = []
    for path, entry in lock.get("packages", {}).items():
        if not path or entry.get("dev") or entry.get("devOptional"):
            continue
        name = path.removeprefix("node_modules/")
        licence = entry.get("license")
        if not isinstance(licence, str) or not licence.strip():
            refusals.append(f"{name} declares no licence")
            continue
        try:
            if not allows(licence, allowed):
                refusals.append(f"{name} is {licence}, which deny.toml does not allow")
        except ValueError as error:
            refusals.append(f"{name}: {error}")
    return refusals


def bundled_refusals(lock: dict, notices: str) -> list[str]:
    """The packages Vite's notices name as bundled that the lockfile marks for development."""
    packages = lock.get("packages", {})
    refusals = []
    for name in re.findall(r"^## (\S+) - \S+ \(", notices, re.MULTILINE):
        entry = packages.get(f"node_modules/{name}", {})
        if entry.get("dev") or entry.get("devOptional"):
            refusals.append(f"{name} ships in the bundle, and package-lock.json marks it dev: move it to dependencies")
    return refusals


def about_refusals(about: dict, allowed: list[str]) -> list[str]:
    """How about.toml's accepted list differs from deny.toml's allow-list."""
    accepted = about.get("accepted", [])
    refusals = []
    for licence in sorted(set(allowed) - set(accepted)):
        refusals.append(f"about.toml does not accept {licence}, which deny.toml allows")
    for licence in sorted(set(accepted) - set(allowed)):
        refusals.append(f"about.toml accepts {licence}, which deny.toml does not allow")
    return refusals


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--bundle", type=pathlib.Path, help="the third-party notices of a frontend build")
    args = parser.parse_args()
    deny = tomllib.loads((ROOT / "backend/deny.toml").read_text())
    allowed = deny["licenses"]["allow"]
    lock = json.loads((ROOT / "frontend/package-lock.json").read_text())
    about = tomllib.loads((ROOT / "backend/about.toml").read_text())

    refusals = npm_refusals(lock, set(allowed)) + about_refusals(about, allowed)
    if args.bundle:
        bundled = bundled_refusals(lock, args.bundle.read_text())
        if not re.search(r"^## ", args.bundle.read_text(), re.MULTILINE):
            bundled.append(f"{args.bundle} names no package: the notices' shape changed")
        refusals += bundled
    for refusal in refusals:
        print(f"::error::{refusal}")
    if refusals:
        print("A new licence is added to backend/deny.toml by a person, on purpose,")
        print("and to backend/about.toml with it.")
        return 1
    print("Every bundled package and about.toml keep to deny.toml's allow-list.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
