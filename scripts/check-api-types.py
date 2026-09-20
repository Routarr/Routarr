#!/usr/bin/env python3
"""Check that the frontend's view of a response matches the backend's.

The API contract is written twice: once as `#[derive(Serialize)]` structs in
`backend/src/`, once as interfaces in `frontend/src/api/types.ts`. Nothing joins
them. Rename a field in Rust and everything still compiles, `cargo test` passes,
`svelte-check` passes, and the interface renders `undefined` — the only layer
that could notice is an end-to-end journey that happens to read that field.

This closes the common half of that gap: for each pair declared in `PAIRS`
below, the serialized field names must be identical on both sides.

What it does *not* do, stated plainly so nobody trusts it further than it goes:

* It compares **names**, not types. `count: number` against `count: String`
  passes here and breaks at run time.
* `PAIRS` is hand-written. A response struct with no entry is reported, not
  compared — the list at the end of a run is the honest measure of coverage.
* It reads declarations, not traffic. A handler that assembles a payload with
  `serde_json::json!` instead of a struct is invisible to it.

Run from anywhere: it resolves its own paths. Exits non-zero on a mismatch.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST = ROOT / "backend" / "src"
TYPES = ROOT / "frontend" / "src" / "api" / "types.ts"

# Rust struct -> TypeScript interface.
#
# The names differ on purpose: Rust says `InstanceResponse` because it is what a
# handler returns, TypeScript says `Instance` because it is what a component
# holds. Adding a response type means adding a line here — which is the point,
# since the alternative is a type nobody notices is unmirrored.
PAIRS: dict[str, str] = {
    "InstanceResponse": "Instance",
    "RootFolderWithInstance": "RootFolder",
    "CategoryWithUsage": "Category",
    "OverrideWithMedia": "OverrideEntry",
    "MediaListItem": "MediaListItem",
    "LogEntry": "LogEntry",
    "Job": "Job",
    "MappingConflict": "MappingConflict",
    "RuleTest": "RuleTest",
    "BackupFile": "BackupFile",
    "BackupListResponse": "BackupList",
    "HealthResponse": "Health",
    "LocalizationResponse": "Localization",
    "PreviewResponse": "RulePreview",
    "ProvidersResponse": "MetadataProviders",
    "StatusResponse": "Status",
    "RestoreResponse": "RestoreResult",
    "BackupManifest": "BackupManifest",
    "TestConnectionResponse": "TestConnectionResponse",
}


def strip_comments(text: str) -> str:
    """Remove `//` and `/* */`, leaving string literals alone.

    A naive strip would eat the `//` inside `"https://…"`, which appears in the
    doc comments of both files.
    """
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            out.append(c)
            i += 1
            while i < n and text[i] != '"':
                if text[i] == "\\":
                    out.append(text[i])
                    i += 1
                if i < n:
                    out.append(text[i])
                    i += 1
            if i < n:
                out.append(text[i])
                i += 1
        elif text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
        elif text.startswith("/*", i):
            i = text.find("*/", i)
            i = n if i < 0 else i + 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def rust_structs(root: Path) -> dict[str, set[str]]:
    """Serialized field names of every `Serialize` struct, by struct name."""
    found: dict[str, set[str]] = {}
    for path in sorted(root.rglob("*.rs")):
        source = strip_comments(path.read_text())
        # `derive(...)` on the line(s) before `pub struct X {`.
        for match in re.finditer(
            r"#\[derive\(([^)]*)\)\]((?:\s*#\[[^\]]*\])*)\s*pub struct (\w+)\s*\{([^}]*)\}",
            source,
            re.S,
        ):
            derives, attrs, name, body = match.groups()
            if "Serialize" not in derives:
                continue

            fields: set[str] = set()
            # Each field, with the attributes attached to it.
            for field in re.finditer(
                r"((?:#\[[^\]]*\]\s*)*)pub (\w+)\s*:", body
            ):
                field_attrs, field_name = field.groups()
                # `skip_serializing` drops the field; `skip_serializing_if`
                # keeps it and only omits it when the predicate holds, so it is
                # still part of the contract. The two differ by a suffix, which
                # is why this is a regex and not a substring test.
                if re.search(r"\bskip(_serializing)?\b(?!_if)", field_attrs):
                    continue
                # `flatten` splices another struct's fields in. The type is
                # recorded and spliced once every struct is known — resolved
                # here, a struct defined later in the walk would be missing.
                if "flatten" in field_attrs:
                    flattened = re.search(
                        r"pub " + field_name + r"\s*:\s*([A-Za-z_][A-Za-z0-9_]*)", body
                    )
                    fields.add(f"__flatten__:{flattened.group(1)}" if flattened else "__flatten__")
                    continue
                rename = re.search(r'rename\s*=\s*"([^"]+)"', field_attrs)
                fields.add(rename.group(1) if rename else field_name)

            if "rename_all" in attrs:
                fields.add("__rename_all__")
            found[name] = fields

    return resolve_flattened(found)


def resolve_flattened(structs: dict[str, set[str]]) -> dict[str, set[str]]:
    """Splice each `#[serde(flatten)]` field into its own struct's field set.

    The three composite responses are exactly the ones a reader is most likely
    to get wrong — `CLAUDE.md` warns to consume them as flat objects — and they
    were the three the check gave up on, while its summary still counted them
    as agreeing. A flattened field is not unresolvable: the struct it names is
    in this same map.

    A type the walk never saw (one outside `backend/src`, or a generic) leaves
    the marker in place, so the caller still skips rather than comparing a set
    it knows to be short. Nesting resolves by repeating until nothing moves;
    a cycle cannot exist, since serde would not compile it.
    """
    for _ in range(len(structs) + 1):
        moved = False
        for name, fields in structs.items():
            for marker in [f for f in fields if f.startswith("__flatten__:")]:
                inner = structs.get(marker.split(":", 1)[1])
                if inner is None or any(f.startswith("__flatten__") for f in inner):
                    continue
                fields.discard(marker)
                fields |= inner
                moved = True
        if not moved:
            break
    return structs


def ts_interfaces(path: Path) -> dict[str, set[str]]:
    """Top-level field names of every exported interface, by interface name.

    Depth matters. Several interfaces inline their nested shapes rather than
    naming them — `instances: { id: string; name: string; … }[]` in `Health` —
    and a line-by-line read would collect `id` and `name` as if the response
    carried them at the top. The Rust side has one field there, so every child
    would be reported as missing and the check would cry wolf on a contract that
    agrees.
    """
    source = strip_comments(path.read_text())
    found: dict[str, set[str]] = {}
    for match in re.finditer(r"export interface (\w+)[^{]*\{(.*?)\n\}", source, re.S):
        name, body = match.groups()
        fields = set()
        depth = 0
        for line in body.splitlines():
            if depth == 0:
                field = re.match(r"\s*(\w+)\??\s*:", line)
                if field:
                    fields.add(field.group(1))
            depth += line.count("{") - line.count("}")
        found[name] = fields
    return found


def main() -> int:
    if not TYPES.exists():
        print(f"check-api-types: {TYPES} not found", file=sys.stderr)
        return 2

    rust = rust_structs(RUST)
    ts = ts_interfaces(TYPES)
    failures: list[str] = []
    skipped: list[str] = []

    for rust_name, ts_name in sorted(PAIRS.items()):
        if rust_name not in rust:
            failures.append(f"{rust_name}: no Serialize struct by that name in backend/src")
            continue
        if ts_name not in ts:
            failures.append(f"{ts_name}: no exported interface by that name in types.ts")
            continue

        rust_fields = rust[rust_name]
        unresolved = [f for f in rust_fields if f.startswith("__flatten__")]
        if unresolved or "__rename_all__" in rust_fields:
            reason = (
                f"flattens {unresolved[0].split(':', 1)[-1]}, which is not in backend/src"
                if unresolved
                else "renames every field through rename_all"
            )
            print(f"  skipped {rust_name}: {reason}")
            skipped.append(rust_name)
            continue

        ts_fields = ts[ts_name]
        missing_in_ts = rust_fields - ts_fields
        missing_in_rust = ts_fields - rust_fields

        if missing_in_ts:
            failures.append(
                f"{rust_name} -> {ts_name}: the backend sends "
                f"{sorted(missing_in_ts)}, the interface does not declare it"
            )
        if missing_in_rust:
            failures.append(
                f"{rust_name} -> {ts_name}: the interface expects "
                f"{sorted(missing_in_rust)}, the backend does not send it"
            )

    # Coverage, reported rather than enforced: a response type with no pair is
    # not a failure, it is an unmirrored type someone should decide about.
    unpaired = sorted(
        name
        for name in rust
        if name.endswith("Response") and name not in PAIRS
    )
    if unpaired:
        print(f"  {len(unpaired)} response struct(s) with no declared pair: {', '.join(unpaired)}")

    if failures:
        print("\ncheck-api-types: the two sides of the contract disagree\n", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1

    # Compared, not declared. Saying "19 pairs agree" while three of them were
    # skipped is how a check comes to be trusted for work it did not do — and
    # the ones it gives up on are the composite responses, the hardest to get
    # right by reading.
    compared = len(PAIRS) - len(skipped)
    tail = f", {len(skipped)} skipped ({', '.join(skipped)})" if skipped else ""
    print(f"check-api-types: {compared} pair(s) compared and agreeing{tail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
