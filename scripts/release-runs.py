#!/usr/bin/env python3
"""Whether the two workflows that test a commit passed on it, which
`scripts/release-guard.sh` asks before a release publishes the commit.

    gh api "repos/OWNER/REPOSITORY/actions/runs?head_sha=SHA&per_page=100" \\
      | python3 scripts/release-runs.py

Reads the runs API's answer on stdin, prints each required run, and exits 1
naming the one missing or not green. Only a run of `ci.yml` and one of
`docker.yml` started by a push count. Every other run on the commit is left
out: the site's weekly comparison, CodeQL, a Dependabot update job and the
release itself say nothing about whether the commit works.
"""

import json
import sys

REQUIRED = {
    ".github/workflows/ci.yml": "CI",
    ".github/workflows/docker.yml": "Docker image",
}


def refusals(answer: dict) -> list[str]:
    """Why the commit may not be published, nothing when it may."""
    latest: dict[str, dict] = {}
    for run in answer.get("workflow_runs", []):
        path = run.get("path")
        if path not in REQUIRED or run.get("event") != "push":
            continue
        # A run started again keeps its id. A second push of the commit starts
        # another, and the newest answers for it.
        if path not in latest or run["id"] > latest[path]["id"]:
            latest[path] = run
    reasons = []
    for path, name in REQUIRED.items():
        run = latest.get(path)
        if run is None:
            reasons.append(f"No {name} run on a push of this commit.")
        elif run.get("conclusion") != "success":
            state = run.get("conclusion") or run.get("status")
            reasons.append(f"{name} is {state}, not success: {run.get('html_url', '')}")
    return reasons


def main() -> int:
    answer = json.load(sys.stdin)
    reasons = refusals(answer)
    for reason in reasons:
        print(reason)
    if not reasons:
        print("CI and Docker image both passed on a push of this commit.")
    return 1 if reasons else 0


if __name__ == "__main__":
    sys.exit(main())
