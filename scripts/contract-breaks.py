#!/usr/bin/env python3
"""What `scripts/check-api-breaks.sh` asks of the contract besides oasdiff.

    python3 scripts/contract-breaks.py [--routes MAIN.rs] RELEASED.json CURRENT.json [ACCEPTED.txt]

oasdiff compares the operations, and reads neither the scope an operation is
granted under nor a schema no operation returns. Two promises live there:

- `x-routarr-scope` is part of what an application key was given. Raised, it
  refuses a client the release let in. Lowered, it hands every key already
  issued a reach nobody granted it.
- The notification webhook receives a `Notification`, and no operation returns
  one. A receiver reads it whole, the schemas it names included: a property
  removed, a type changed or a field no longer required breaks it.
- A deprecated path is kept for the clients of a release. One no release
  published keeps nobody's script working, and is removed instead. MAIN.rs,
  the release's route table, adds the owner's routes its contract leaves out.

Prints one line per break and exits 1 when there is any. A break written whole as a line of
ACCEPTED, the release's list of breaks it names in its notes, is not one.
"""

import json
import re
import sys

PAYLOADS = ["Notification"]
REF = "#/components/schemas/"


def scope_changes(released: dict, current: dict) -> list[str]:
    def scopes(contract: dict) -> dict[str, str]:
        return {
            f"{method.upper()} {route}": operation.get("x-routarr-scope")
            for route, item in contract.get("paths", {}).items()
            for method, operation in item.items()
            if isinstance(operation, dict)
        }

    before, after = scopes(released), scopes(current)
    return [
        f"x-routarr-scope changed on {operation}: {scope} in the release, {after[operation]} now"
        for operation, scope in sorted(before.items())
        if scope and operation in after and after[operation] != scope
    ]


def routed(main: str) -> set[str]:
    """The paths a route table (`backend/src/main.rs`) mounts, under `/api/v1`."""
    return set(re.findall(r'\.route\(\s*"(/[^"]*)"', main))


def unreleased_deprecations(released: dict, current: dict, routes: set[str] = frozenset()) -> list[str]:
    """The deprecated operations of `current` whose path neither `released` nor its `routes` held."""
    published = set(released.get("paths", {})) | set(routes)
    return [
        f"{method.upper()} {route} is deprecated, and no release published it: remove it"
        for route, item in sorted(current.get("paths", {}).items())
        for method, operation in item.items()
        if isinstance(operation, dict) and operation.get("deprecated") and route not in published
    ]


def payload_breaks(released: dict, current: dict, name: str) -> list[str]:
    """How the current contract breaks a receiver of the released `name`."""

    def resolve(contract: dict, schema: dict) -> tuple[str | None, dict | None]:
        ref = schema.get("$ref")
        if not ref:
            return None, schema
        target = ref.removeprefix(REF)
        return target, contract.get("components", {}).get("schemas", {}).get(target)

    breaks: list[str] = []
    seen: set[tuple[str | None, str | None]] = set()

    def compare(before: dict, after: dict, where: str) -> None:
        before_ref, before = resolve(released, before)
        after_ref, after = resolve(current, after)
        if before is None:
            return
        if after is None:
            breaks.append(f"{where} names a schema the contract no longer documents")
            return
        if before_ref and after_ref:
            if (before_ref, after_ref) in seen:
                return
            seen.add((before_ref, after_ref))
        for facet in ("type", "format"):
            if facet in before and before.get(facet) != after.get(facet):
                breaks.append(
                    f"{where} changes its {facet} from {before[facet]} to {after.get(facet)}"
                )
        before_fields = before.get("properties", {})
        after_fields = after.get("properties", {})
        for field in sorted(before_fields):
            if field not in after_fields:
                breaks.append(f"{where}.{field} is in the release's contract and no longer documented")
            else:
                compare(before_fields[field], after_fields[field], f"{where}.{field}")
        for field in sorted(set(before.get("required", [])) - set(after.get("required", []))):
            if field in after_fields:
                breaks.append(f"{where}.{field} is no longer required, so a receiver may miss it")
        if "items" in before and "items" in after:
            compare(before["items"], after["items"], f"{where}[]")

    compare({"$ref": REF + name}, {"$ref": REF + name}, name)
    return breaks


def unaccepted(problems: list[str], accepted: str) -> list[str]:
    """The `problems` no line of `accepted` names whole."""
    lines = {line.strip() for line in accepted.splitlines()}
    return [problem for problem in problems if problem not in lines]


def main(argv: list[str]) -> int:
    routes: set[str] = set()
    if len(argv) > 2 and argv[1] == "--routes":
        with open(argv[2]) as file:
            routes = routed(file.read())
        argv = [argv[0], *argv[3:]]
    with open(argv[1]) as file:
        released = json.load(file)
    with open(argv[2]) as file:
        current = json.load(file)
    problems = scope_changes(released, current) + unreleased_deprecations(released, current, routes)
    for name in PAYLOADS:
        problems += payload_breaks(released, current, name)
    if len(argv) > 3:
        with open(argv[3]) as file:
            problems = unaccepted(problems, file.read())
    for problem in problems:
        print(f"error: {problem}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
