"""`scripts/release-runs.py`, the release guard's verdict on a commit's runs."""

import importlib.util
import pathlib
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parent.parent / "release-runs.py"
spec = importlib.util.spec_from_file_location("release_runs", SCRIPT)
release_runs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_runs)


def run(path, conclusion="success", *, event="push", id=1, status="completed"):
    return {
        "id": id,
        "path": path,
        "event": event,
        "status": status,
        "conclusion": conclusion,
        "html_url": f"https://github.com/runs/{id}",
    }


CI = ".github/workflows/ci.yml"
DOCKER = ".github/workflows/docker.yml"


def refusals(*runs):
    return release_runs.refusals({"workflow_runs": list(runs)})


class ReleaseRuns(unittest.TestCase):
    def test_a_commit_green_in_both_workflows_may_be_published(self):
        self.assertEqual(refusals(run(CI, id=1), run(DOCKER, id=2)), [])

    def test_a_run_that_tests_nothing_neither_blocks_nor_stands_in(self):
        others = [
            run("dynamic/dependabot/dependabot-updates", "failure", event="dynamic", id=3),
            run(".github/workflows/site.yml", "failure", event="schedule", id=4),
            run("dynamic/github-code-scanning/codeql", "failure", event="dynamic", id=5),
            run(".github/workflows/release.yml", None, status="in_progress", id=6),
        ]
        self.assertEqual(refusals(run(CI, id=1), run(DOCKER, id=2), *others), [])
        self.assertEqual(len(refusals(*others)), 2)

    def test_a_commit_ci_never_ran_on_is_refused(self):
        reasons = refusals(run(DOCKER, id=2))
        self.assertEqual(reasons, ["No CI run on a push of this commit."])

    def test_a_pull_request_run_does_not_stand_in_for_the_push(self):
        reasons = refusals(run(CI, event="pull_request", id=1), run(DOCKER, id=2))
        self.assertEqual(reasons, ["No CI run on a push of this commit."])

    def test_a_failed_or_unfinished_run_is_refused(self):
        failed = refusals(run(CI, "failure", id=1), run(DOCKER, id=2))
        self.assertEqual(len(failed), 1)
        self.assertIn("CI is failure", failed[0])
        running = refusals(run(CI, id=1), run(DOCKER, None, status="in_progress", id=2))
        self.assertEqual(len(running), 1)
        self.assertIn("Docker image is in_progress", running[0])

    def test_a_skipped_or_cancelled_run_is_refused(self):
        for conclusion in ["skipped", "cancelled"]:
            reasons = refusals(run(CI, conclusion, id=1), run(DOCKER, id=2))
            self.assertEqual(len(reasons), 1, conclusion)

    def test_the_newest_run_of_a_workflow_answers_for_it(self):
        self.assertEqual(refusals(run(CI, "failure", id=1), run(CI, id=7), run(DOCKER, id=2)), [])
        reasons = refusals(run(CI, id=1), run(CI, "failure", id=7), run(DOCKER, id=2))
        self.assertEqual(len(reasons), 1)


if __name__ == "__main__":
    unittest.main()
