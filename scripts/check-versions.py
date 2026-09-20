#!/usr/bin/env python3
"""Check that a version written in several files is the same in all of them.

Two of them, in fact: the toolchain versions and Routarr's own.

`CLAUDE.md` says the compiler version lives in `rust-toolchain.toml` and nowhere
else. That is the intent, not the fact: rustup reads the file, but the two
Dockerfiles have to name a base image tag, and `Cargo.toml` states the MSRV. The
version is written four times for Rust and five for Node, and nothing joined
them — a bump that updates three of four leaves an environment behind, and the
gap only shows as a build that fails in one place and passes everywhere else.

That gap became a certainty the day Dependabot started watching the `docker`
ecosystem: it moves the `FROM` line and *only* the `FROM` line. This turns the
drift it would otherwise introduce into a failed check that names the files
still holding the old value.

The product version has the same shape of problem for a different reason. The
release reads `Cargo.toml` — the tag names that value and `site/check.mjs`
asserts the showcase page states it — while the two `package.json` files carry
it as well and nothing read them. A release could ship a front end announcing
the version before it.

Run from anywhere: it resolves its own paths. Exits non-zero on a mismatch.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def read(relative: str) -> str:
    return (ROOT / relative).read_text()


# The Dockerfiles pin their base images as `image:tag@digest`. The digest is what
# Docker pulls; the tag is what these patterns read, and what Dependabot tracks —
# it updates the two together. A digest alone would be invisible to Dependabot,
# which has no tag to follow, and the `docker` pull requests this script exists
# to catch would simply stop arriving. The tag cannot live in a comment beside
# the `FROM` either: Dockerfile has no trailing comments (see `from_lines_parse`).


def find(relative: str, pattern: str) -> tuple[str, str] | None:
    """The first capture of `pattern` in a file, with a label for the report."""
    match = re.search(pattern, read(relative), re.M)
    return (relative, match.group(1)) if match else None


def from_lines_parse() -> list[str]:
    """Every `FROM` is one Docker will accept.

    Dockerfile has no trailing comments: a `#` is a comment only at the start of
    a line, and anything after the image on a `FROM` line is read as arguments.
    `FROM image@sha256:… AS stage  # tag` is therefore four or five arguments
    where Docker accepts one or three, and the build fails on its first line.

    It is checked here because this script already reads both Dockerfiles, and
    because nothing else in the repository parses them without a Docker daemon —
    the dev container has none, so the first place this failure surfaced was the
    image build in CI, on a push, after every other gate had passed. The
    devcontainer's own Dockerfile is not built by CI at all, so there it would
    have surfaced as a container that refuses to rebuild.
    """
    problems: list[str] = []
    for relative in ("Dockerfile", ".devcontainer/Dockerfile"):
        for number, line in enumerate(read(relative).splitlines(), 1):
            if not line.startswith("FROM "):
                continue
            arguments = line.split()[1:]
            # `FROM image` or `FROM image AS stage`.
            well_formed = len(arguments) == 1 or (
                len(arguments) == 3 and arguments[1].upper() == "AS"
            )
            if not well_formed:
                trailing = "#" in arguments
                problems.append(
                    f"{relative}:{number}: FROM must be `image` or `image AS stage`"
                    + (
                        " — this line carries a trailing comment, which Dockerfile "
                        "reads as arguments; put the tag on the line above"
                        if trailing
                        else f", and this one has {len(arguments)} arguments"
                    )
                )
    return problems


def agree(what: str, sightings: list[tuple[str, str] | None]) -> list[str]:
    """Every sighting must carry the same value. Returns the failures."""
    missing = [
        f"{what}: a pattern matched nothing — the file moved or its shape changed"
        for sighting in sightings
        if sighting is None
    ]
    found = [s for s in sightings if s is not None]
    values = {value for _, value in found}

    if missing:
        return missing
    if len(values) <= 1:
        return []

    # The report is the list of who says what: a bump that updated three files
    # out of four is read at a glance, which a "they differ" would not be.
    lines = [f"{what} is written {len(found)} times and they do not agree:"]
    lines += [f"    {value:<12} {path}" for path, value in sorted(found, key=lambda s: s[1])]
    return lines


def main() -> int:
    failures: list[str] = []

    # --- Rust ---------------------------------------------------------------
    # `rust-toolchain.toml` is what rustup reads, so it is the value the others
    # have to match. `Cargo.toml` states the MSRV as `x.y` rather than `x.y.z`,
    # which is not a disagreement — it is the same version, less precisely — so
    # it is compared against the channel's own prefix.
    channel = find("backend/rust-toolchain.toml", r'^channel\s*=\s*"([\d.]+)"')
    failures += agree(
        "The Rust version",
        [
            channel,
            find("Dockerfile", r"^FROM rust:([\d.]+)-alpine@sha256:[0-9a-f]{64}"),
            find(".devcontainer/Dockerfile", r"^FROM rust:([\d.]+)@sha256:[0-9a-f]{64}"),
        ],
    )

    msrv = find("backend/Cargo.toml", r'^rust-version\s*=\s*"([\d.]+)"')
    if channel and msrv:
        wanted = ".".join(channel[1].split(".")[:2])
        if msrv[1] != wanted:
            failures.append(
                f"The MSRV in {msrv[0]} is {msrv[1]}, but the pinned toolchain is "
                f"{channel[1]} — it should read {wanted}"
            )

    # --- Node ---------------------------------------------------------------
    # Major only, everywhere: nothing here pins a minor, and CI resolves the
    # latest of the line. Five sightings, and the CI workflow holds three of
    # them on its own.
    ci = read(".github/workflows/ci.yml")
    ci_versions = sorted(set(re.findall(r"^\s*node-version:\s*(\d+)", ci, re.M)))
    node = [
        find("Dockerfile", r"^FROM node:(\d+)-alpine@sha256:[0-9a-f]{64}"),
        find(".devcontainer/Dockerfile", r"^ARG NODE_VERSION=(\d+)\."),
        find("site/.node-version", r"^(\d+)"),
    ]
    if len(ci_versions) > 1:
        failures.append(
            f"The CI workflow asks for several Node versions: {', '.join(ci_versions)}"
        )
    elif ci_versions:
        node.append((".github/workflows/ci.yml", ci_versions[0]))
    failures += agree("The Node major version", node)

    # --- Playwright -----------------------------------------------------------
    # The dev container installs the browser for one version and the frontend
    # runs the tests with another: a Dependabot bump of `@playwright/test`
    # alone leaves `install-deps` fetching for a release the lockfile no longer
    # names, which surfaces as a browser that will not launch.
    failures += agree(
        "The Playwright version",
        [
            find(".devcontainer/Dockerfile", r"^ARG PLAYWRIGHT_VERSION=([\d.]+)"),
            find(
                "frontend/package-lock.json",
                r'"node_modules/@playwright/test":\s*\{\s*"version":\s*"([\d.]+)"',
            ),
        ],
    )

    # --- Routarr's own version ----------------------------------------------
    # `Cargo.toml` is the one that counts: the tag names it, and the showcase
    # page is checked against it. The other two are carried along, so the only
    # question is whether a bump reached them.
    product = find("backend/Cargo.toml", r'^version\s*=\s*"([^"]+)"')
    failures += agree(
        "The Routarr version",
        [
            product,
            find("frontend/package.json", r'^\s*"version":\s*"([^"]+)"'),
            find("site/package.json", r'^\s*"version":\s*"([^"]+)"'),
        ],
    )

    failures += from_lines_parse()

    if failures:
        print("check-versions: a version is not the same everywhere\n", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1

    print(
        f"check-versions: Routarr {product[1] if product else '?'}, Rust "
        f"{channel[1] if channel else '?'} and Node "
        f"{ci_versions[0] if ci_versions else '?'} agree everywhere they are written"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
