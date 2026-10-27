"""Run locally what CI runs, for the areas a branch changed, read from `.github/workflows/ci.yml`.

The commands are never copied here: each `run:` step of the jobs below is read from the
workflow and run in its working directory, so the two cannot drift. A step only CI can take
(installing, uploading, deciding) is skipped and said so, and so is a linter not installed.

    bash scripts/check.sh [--changed [BASE]] [--fail-fast] [--engines LIST] [--list] [AREA...]

Exit status: 0 every step passed, 1 a step failed, 2 a usage error, 3 passed with steps skipped.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

# The area each job of ci.yml belongs to, in the order they run. `changes` holds one check
# that must see every commit, which belongs with the repository's.
AREAS: dict[str, str] = {
    "changes": "repository",
    "repository": "repository",
    "backend": "backend",
    "coverage": "coverage",
    "frontend": "frontend",
    "site": "site",
    "e2e": "e2e",
}
ORDER = ["repository", "backend", "coverage", "frontend", "site", "e2e"]

# Jobs that decide or gather and run nothing to check.
CI_ONLY_JOBS = {"e2e-done"}

# Steps only CI takes, by the start of their command, each with why.
CI_ONLY_STEPS = {
    "npm ci": "installs the packages: the dev container's post-create does it",
    "bash scripts/install-browser.sh": "installs a browser: the dev container's image and post-create do it",
    "set -euo pipefail\nmkdir -p": "downloads the pinned linters: the dev container has them",
}

# GitHub's shell for a `run:` that names none: a multi-line step stops at its first failure.
SHELL = ["bash", "-e", "-c"]

# Docker builds the image and the dev container, which the dev container cannot run.
DOCKER_AREAS = {"image": "docker.yml builds and smoke-tests the image", "devcontainer": "docker.yml builds the dev container"}


@dataclass
class Step:
    job: str
    name: str
    run: str
    directory: str
    condition: str = ""
    env: dict[str, str] = field(default_factory=dict)

    @property
    def area(self) -> str:
        return AREAS[self.job]

    def ci_only(self) -> str | None:
        """Why only CI takes this step, or None."""
        for start, why in CI_ONLY_STEPS.items():
            if self.run.startswith(start):
                return why
        if "${{" in self.run:
            return "reads a value only the workflow has"
        if "$GITHUB_" in self.run:
            return "decides from the run's GitHub context: this script asks changed-areas.sh"
        return None

    def missing_tool(self) -> str | None:
        """The linter this step needs and the machine lacks, when its job installs it."""
        if "steps.tools.outcome" not in self.condition:
            return None
        words = [word for word in re.split(r"[\s|]+", self.run) if word]
        tool = next((w for w in words if w in {"actionlint", "shellcheck", "ruff", "hadolint", "gitleaks"}), words[0])
        return None if shutil.which(tool) else tool


def unquoted(value: str) -> str:
    value = value.strip()
    return value[1:-1] if len(value) > 1 and value[0] == value[-1] and value[0] in "'\"" else value


def parse(text: str) -> list[Step]:
    """Every `run:` step of every job, with its working directory, condition and environment.
    Reads the subset of YAML ci.yml writes: two-space indents, `run: |` blocks,
    `working-directory`, and `env:` on a job or a step."""
    steps: list[Step] = []
    lines = text.split("\n")
    job = None
    default_dir = "."
    env: dict[str, str] = {}
    step_env: dict[str, str] = {}
    current: dict[str, str] | None = None
    block_key = None
    block_indent = 0
    in_env = False
    in_step_env = False

    def close() -> None:
        if job and current and "run" in current:
            steps.append(
                Step(
                    job=job,
                    name=current.get("name", current["run"].split("\n")[0]),
                    run=current["run"].rstrip("\n"),
                    directory=current.get("working-directory", default_dir),
                    condition=current.get("if", ""),
                    env={**env, **step_env},
                )
            )

    in_jobs = False
    for line in lines:
        if block_key is not None:
            if line.strip() == "" or len(line) - len(line.lstrip()) >= block_indent:
                assert current is not None
                current[block_key] += line[block_indent:] + "\n"
                continue
            block_key = None
        if line.startswith("jobs:"):
            in_jobs = True
            continue
        if not in_jobs:
            continue
        job_match = re.match(r"^  ([a-z0-9-]+):\s*$", line)
        if job_match:
            close()
            current = None
            job = job_match.group(1)
            default_dir = "."
            env = {}
            in_env = False
            continue
        if re.match(r"^    env:\s*$", line):
            in_env = True
            continue
        env_match = re.match(r"^      ([A-Z][A-Z0-9_]*): (.*)$", line)
        if in_env and env_match:
            env[env_match.group(1)] = unquoted(env_match.group(2))
            continue
        step_env_match = re.match(r"^          ([A-Z][A-Z0-9_]*): (.*)$", line)
        if in_step_env and step_env_match:
            step_env[step_env_match.group(1)] = unquoted(step_env_match.group(2))
            continue
        in_step_env = False
        if re.match(r"^    \S", line):
            in_env = False
        dir_match = re.match(r"^\s{8}working-directory: (.+)$", line)
        if dir_match and current is None:
            default_dir = dir_match.group(1).strip()
            continue
        item = re.match(r"^      - (?:([a-z-]+): ?(.*))?$", line)
        if item:
            close()
            current = {}
            step_env = {}
            if item.group(1):
                key, value = item.group(1), item.group(2)
                if value in ("|", ">"):
                    current[key] = ""
                    block_key, block_indent = key, 10
                else:
                    current[key] = value.strip()
            continue
        key_match = re.match(r"^        ([a-z-]+): ?(.*)$", line)
        if key_match and current is not None:
            key, value = key_match.group(1), key_match.group(2)
            if key == "env" and not value:
                in_step_env = True
            elif value in ("|", ">"):
                current[key] = ""
                block_key, block_indent = key, 10
            else:
                current[key] = unquoted(value) if key != "if" else value.strip()
    close()
    return [step for step in steps if step.job not in CI_ONLY_JOBS]


def changed_areas(base: str | None) -> set[str]:
    """The areas `changed-areas.sh` wakes for this branch and its uncommitted work."""
    if base is None:
        found = subprocess.run(
            ["git", "merge-base", "HEAD", "origin/main"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        base = found.stdout.strip()
        if not base:
            print("check: no origin/main to compare with, so every area runs (or pass --changed BASE)", file=sys.stderr)
    answer = subprocess.run(
        ["bash", "scripts/changed-areas.sh", base, "WORKTREE"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    lines = dict(line.split("=", 1) for line in answer.stdout.split("\n") if "=" in line)
    for name in ("backend", "frontend", "site", "image", "devcontainer"):
        if name not in lines:
            sys.exit(f"check: changed-areas.sh printed no {name} line")
    areas = {"repository"} | {name for name, value in lines.items() if value == "true"}
    if "backend" in areas:
        areas.add("coverage")
    if areas & {"backend", "frontend"}:
        areas.add("e2e")
    return areas


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("areas", nargs="*", metavar="AREA", help=f"{', '.join(ORDER)}, image, devcontainer or all")
    parser.add_argument("--changed", nargs="?", const="", metavar="BASE", help="the areas changed since BASE (the merge base with origin/main)")
    parser.add_argument("--fail-fast", action="store_true", help="stop at the first failure")
    parser.add_argument("--engines", default="chromium", help="the browsers the end-to-end suites run in, comma-separated")
    parser.add_argument("--list", action="store_true", help="print each step as area, directory, command, and run nothing")
    args = parser.parse_args()

    steps = parse(WORKFLOW.read_text())
    if args.list:
        for step in steps:
            why = step.ci_only()
            command = step.run.replace("\n", "; ")
            print(f"{step.area}\t{step.directory}\t{command}" + (f"\t(CI only: {why})" if why else ""))
        return 0

    known = set(ORDER) | set(DOCKER_AREAS) | {"all"}
    unknown = [area for area in args.areas if area not in known]
    if unknown:
        parser.error(f"unknown area {', '.join(unknown)}")
    if "all" in args.areas:
        wanted = set(ORDER) | set(DOCKER_AREAS)
    elif args.areas and args.changed is None:
        wanted = set(args.areas)
    else:
        wanted = changed_areas(args.changed or None) | set(args.areas)

    passed: list[str] = []
    failed: list[str] = []
    skipped: list[str] = []
    for area, why in DOCKER_AREAS.items():
        if area in wanted:
            skipped.append(f"{area}: {why}, which needs Docker")
    engines = [engine.strip() for engine in args.engines.split(",") if engine.strip()]
    for area in [a for a in ORDER if a in wanted]:
        for step in [s for s in steps if s.area == area]:
            label = f"{area}: {step.name}"
            why = step.ci_only()
            if why:
                print(f"skip  {label} ({why})")
                continue
            tool = step.missing_tool()
            if tool:
                skipped.append(f"{label}: {tool} is not installed")
                continue
            for engine in engines if area == "e2e" else [""]:
                env = {**os.environ, **{k: v for k, v in step.env.items() if "${{" not in v}}
                named = label + (f" ({engine})" if engine else "")
                if engine:
                    env["ROUTARR_E2E_BROWSER"] = engine
                print(f"\n==> {named}\n    {step.run.replace(chr(10), chr(10) + '    ')}", flush=True)
                started = time.monotonic()
                ran = subprocess.run([*SHELL, step.run], cwd=ROOT / step.directory, env=env, check=False)
                spent = f"{time.monotonic() - started:.0f} s"
                (passed if ran.returncode == 0 else failed).append(f"{named} ({spent})")
                if ran.returncode != 0 and args.fail_fast:
                    break
            if failed and args.fail_fast:
                break
        if failed and args.fail_fast:
            break

    print("\nPassed:\n  " + "\n  ".join(passed or ["nothing"]))
    if skipped:
        print("Skipped:\n  " + "\n  ".join(skipped))
    if failed:
        print("Failed:\n  " + "\n  ".join(failed))
        return 1
    return 3 if skipped else 0


if __name__ == "__main__":
    sys.exit(main())
