#!/usr/bin/env python3
"""Check that the frontend's view of a response matches the backend's: each `PAIRS` struct and
its interface in `frontend/src/api/types.ts` carry the same field names. Types are not compared,
and a payload built with `serde_json::json!` escapes the check."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST = ROOT / "backend" / "src"
TYPES = ROOT / "frontend" / "src" / "api" / "types.ts"

# Rust struct -> TypeScript interface. A new response type needs its line here.
PAIRS: dict[str, str] = {
    "InstanceResponse": "Instance",
    "RootFolderWithInstance": "RootFolder",
    "CategoryWithUsage": "Category",
    "OverrideWithMedia": "OverrideEntry",
    "MediaListItem": "MediaListItem",
    "LogEntry": "LogEntry",
    "Job": "Job",
    "MappingConflict": "MappingConflict",
    "OnboardingStatus": "OnboardingStatus",
    "OnboardingStep": "OnboardingStep",
    "RuleTest": "RuleTest",
    "BackupFile": "BackupFile",
    "BackupListResponse": "BackupList",
    "HealthResponse": "Health",
    "LocalizationResponse": "Localization",
    "PreviewResponse": "RulePreview",
    "TemplateSummary": "TemplateSummary",
    "NeededSource": "NeededSource",
    "RequesterTag": "RequesterTag",
    "PlannedRule": "PlannedRule",
    "TakenBy": "TakenBy",
    "TemplateReach": "TemplateReach",
    "TemplateTitle": "TemplateTitle",
    "TemplatePreview": "TemplatePreview",
    "TemplateAdded": "TemplateAdded",
    "ProvidersResponse": "MetadataProviders",
    "StatusResponse": "Status",
    "Warning": "Warning",
    "RestoreResponse": "RestoreResult",
    "BackupManifest": "BackupManifest",
    "TestConnectionResponse": "TestConnectionResponse",
    "Minted": "MintedApplication",
    "ErrorResponse": "ErrorBody",
    "MediaView": "ExplainedMedia",
    "SigningStatus": "WebhookSigningStatus",
    "ImportReport": "ConfigImportReport",
    "ProviderDescription": "MetadataProvider",
    "Listed": "Session",
    # One name on both sides: each is the payload, or a part of one.
    "AlternativeDecision": "AlternativeDecision",
    "ReasonLine": "ReasonLine",
    "Rating": "Rating",
    "ScoreScale": "ScoreScale",
    "Application": "Application",
    "ApplyReport": "ApplyReport",
    "BatchApplyReport": "BatchApplyReport",
    "CapacityForecast": "CapacityForecast",
    "ConditionOutcome": "ConditionOutcome",
    "Decision": "Decision",
    "EnrichmentReport": "EnrichmentReport",
    "Explanation": "Explanation",
    "Facet": "Facet",
    "LibraryFacets": "LibraryFacets",
    "MaintenanceReport": "MaintenanceReport",
    "MediaMetadata": "MediaMetadata",
    "PreviewChange": "PreviewChange",
    "PreviewSummary": "SimulationSummary",
    "Rule": "Rule",
    "ConditionCatalog": "ConditionCatalog",
    "ConditionSpec": "ConditionSpec",
    "RuleHealth": "RuleHealth",
    "RuleHealthReport": "RuleHealthReport",
    "RuleTestResult": "RuleTestResult",
    "RuleTestRun": "RuleTestRun",
    "RuleTrace": "RuleTrace",
    "SecurityEvent": "SecurityEvent",
    "SimulationResult": "SimulationResult",
    "SyncReport": "SyncReport",
    "TitleRefresh": "TitleRefresh",
    "ValidationIssue": "ValidationIssue",
    "Vocabularies": "Vocabularies",
}


# Exported interfaces no struct mirrors, and why.
UNPAIRED: dict[str, str] = {
    "InstanceProbe": "a request body",
    "TemplateRequest": "a request body",
    "KeepRequest": "a request body",
    "DecisionFilter": "a request body",
    "NewApplication": "a request body",
    "Proof": "a request body",
    "PasswordChanged": "built with json! in api::account::change_password",
    "RuleDraft": "a request body",
    "AuthMode": "built with json! in api::auth::mode",
    "Condition": "a tagged enum on the Rust side, which this check does not read",
}


def strip_comments(text: str) -> str:
    """Remove `//` and `/* */`, leaving string literals such as `"https://"` alone."""
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


def rust_structs(root: Path) -> tuple[dict[str, set[str]], dict[str, list[str]]]:
    """Serialized field names of every `Serialize` struct, by struct name, and
    the files of each name more than one struct carries: keyed by name, one of
    them would hide the other."""
    found: dict[str, set[str]] = {}
    files: dict[str, list[str]] = {}
    for path in sorted(root.rglob("*.rs")):
        source = strip_comments(path.read_text())
        # `derive(...)` on the line(s) before `pub struct X {`.
        for match in re.finditer(
            r"#\[derive\(([^)]*)\)\]((?:\s*#\[[^\]]*\])*)\s*pub struct (\w+)\s*\{([^}]*)\}",
            source,
            re.DOTALL,
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
                # Only serde decides what goes on the wire, never `#[sqlx(skip)]`.
                field_attrs = " ".join(re.findall(r"#\[serde\(([^\]]*)\)\]", field_attrs))
                # `skip_serializing` drops a field, where `skip_serializing_if` keeps it.
                if re.search(r"\bskip(_serializing)?\b(?!_if)", field_attrs):
                    continue
                # Spliced once every struct is known, since a later one may define it.
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
            files.setdefault(name, []).append(str(path.relative_to(root)))

    duplicates = {name: paths for name, paths in files.items() if len(paths) > 1}
    return resolve_flattened(found), duplicates


def resolve_flattened(structs: dict[str, set[str]]) -> dict[str, set[str]]:
    """Splice each `#[serde(flatten)]` field into its own struct's field set,
    repeating until nothing moves. A type the walk never saw keeps its marker,
    so the caller skips the pair rather than compare a set it knows is short."""
    for _ in range(len(structs) + 1):
        moved = False
        for fields in structs.values():
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


def top_level_fields(body: str) -> set[str]:
    """The fields declared at the top of a type's body, not those of a shape
    nested inside one of them."""
    fields = set()
    depth = 0
    for line in body.splitlines():
        if depth == 0:
            field = re.match(r"\s*(\w+)\??\s*:", line)
            if field:
                fields.add(field.group(1))
        depth += line.count("{") - line.count("}")
    return fields


def ts_interfaces(path: Path) -> dict[str, set[str]]:
    """Top-level field names of every exported interface or object alias, by
    name. Depth is read, or the inline shape of `Health.instances` would put
    its `id` at the top."""
    source = strip_comments(path.read_text())
    found: dict[str, set[str]] = {}
    bases: dict[str, list[str]] = {}
    for match in re.finditer(
        r"export interface (\w+)(?:\s+extends\s+([\w\s,]+?))?\s*\{(.*?)\n\}", source, re.DOTALL
    ):
        name, extends, body = match.groups()
        found[name] = top_level_fields(body)
        bases[name] = [base.strip() for base in (extends or "").split(",") if base.strip()]
    for match in re.finditer(r"export type (\w+)\s*=\s*\{(.*?)\n\}", source, re.DOTALL):
        name, body = match.groups()
        fields = top_level_fields(body)
        if fields:
            found[name] = fields
            bases[name] = []
    # What an interface extends is part of it, as a flattened struct is in Rust.
    for _ in range(len(found) + 1):
        moved = False
        for name, parents in bases.items():
            for parent in parents:
                inherited = found.get(parent, set()) - found[name]
                if inherited:
                    found[name] |= inherited
                    moved = True
        if not moved:
            break
    return found


def main() -> int:
    if not TYPES.exists():
        print(f"check-api-types: {TYPES} not found", file=sys.stderr)
        return 2

    rust, duplicates = rust_structs(RUST)
    ts = ts_interfaces(TYPES)
    failures: list[str] = []
    skipped: list[str] = []

    for name in sorted(set(duplicates) & set(PAIRS)):
        failures.append(
            f"{name}: a Serialize struct of that name is in {', '.join(duplicates[name])}, "
            "so the pair would compare whichever was read last"
        )

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

    paired = set(PAIRS.values())
    for name in sorted(set(ts) - paired - set(UNPAIRED)):
        failures.append(
            f"{name}: an exported interface with no pair in PAIRS and no reason in UNPAIRED"
        )
    for name in sorted(set(UNPAIRED) - (set(ts) - paired)):
        failures.append(f"{name}: listed in UNPAIRED, but paired or no longer in types.ts")

    # Reported rather than enforced, since most structs never reach the wire.
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

    # A skipped pair is counted apart, never as agreeing.
    compared = len(PAIRS) - len(skipped)
    tail = f", {len(skipped)} skipped ({', '.join(skipped)})" if skipped else ""
    print(f"check-api-types: {compared} pair(s) compared and agreeing{tail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
