#!/usr/bin/env python3
"""What `scripts/check-api-breaks.sh` asks of the contract besides oasdiff.

    python3 scripts/contract-breaks.py RELEASED.json CURRENT.json

oasdiff compares the operations, and reads neither the scope an operation is
granted under nor a schema no operation returns. Two promises live there:

- `x-routarr-scope` is part of what an application key was given. Raised, it
  refuses a client the release let in. Lowered, it hands every key already
  issued a reach nobody granted it.
- The notification webhook receives a `Notification`, and no operation returns
  one. A receiver reads it whole, the schemas it names included: a property
  removed, a type changed or a field no longer required breaks it.

Prints one line per break and exits 1 when there is any.
"""

import json
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


def main(argv: list[str]) -> int:
    with open(argv[1]) as file:
        released = json.load(file)
    with open(argv[2]) as file:
        current = json.load(file)
    problems = scope_changes(released, current)
    for name in PAYLOADS:
        problems += payload_breaks(released, current, name)
    for problem in problems:
        print(f"error: {problem}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
