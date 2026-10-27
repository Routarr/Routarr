"""`scripts/check-commits.py`, the commit rules of CONTRIBUTING.md held on a pull request."""

import os
import shutil
import subprocess
import tempfile
import unittest

from _scripts import SCRIPTS, load

check_commits = load("check-commits")

REFUSED = [
    "test: say why a live move was skipped, and name the live job in the rules",
    "docs(contributing): build the dev container in Docker's format under Podman",
    "build(deps): install Playwright 1.64.0's system libraries in the dev container",
    "fix(site): hold the counts and the gate order the site states to the code",
    "fix(settings): refuse an address without a host and store categories whole",
    "fix(sources): hold a source back as long as it asks, with or without a wait",
    "style(site): open the site in the dark theme",
    "style(site): lift every showcase panel with the same shadow",
    "style(site): square every corner, as the application draws them",
    "style(ui): square every corner",
    "refactor(backup): seal archives with RustCrypto instead of age, and guard the passphrase",
    "Interface: accessibility, languages and a steadier shell",
    "Build the showcase site from the release and hold it to the code",
    "Explain every screen and every card where it is used",
    "fix(site): Hold the counts",
    "fix(site): hold the counts.",
]


class CheckCommits(unittest.TestCase):
    def test_a_subject_breaking_a_rule_is_refused(self):
        for subject in REFUSED:
            with self.subTest(subject):
                self.assertIsNotNone(check_commits.refusal(subject))

    def test_a_subject_keeping_the_rules_passes(self):
        for subject in ["fix(site): hold the counts", "ci: pin the runners", "feat(api)!: drop `/media`",
                        "revert: put the old pin back", "docs: " + "a" * 66]:
            with self.subTest(subject):
                self.assertIsNone(check_commits.refusal(subject))

    def test_the_branch_is_read_without_merges_and_without_dependabot(self):
        folder = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, folder)

        def git(*arguments, author="t"):
            environment = {**os.environ, "GIT_AUTHOR_NAME": author, "GIT_AUTHOR_EMAIL": "t@t",
                           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
            return subprocess.run(["git", *arguments], cwd=folder, env=environment,
                                  capture_output=True, text=True, check=True).stdout.strip()

        git("init", "-q", "-b", "main")
        git("commit", "-q", "--allow-empty", "-m", "Initial commit")
        git("checkout", "-qb", "feature")
        git("commit", "-q", "--allow-empty", "-m", "fix(site): hold the counts")
        git("commit", "-q", "--allow-empty", "-m", "chore(deps): Bump undici", author="dependabot[bot]")
        git("checkout", "-q", "main")
        git("commit", "-q", "--allow-empty", "-m", "Not a conventional subject")
        git("checkout", "-q", "feature")
        git("merge", "-q", "--no-edit", "main")
        git("commit", "-q", "--allow-empty", "-m", "style(site): square every corner")
        ran = subprocess.run(["python3", str(SCRIPTS / "check-commits.py"), "--title", "", "--author", "",
                              git("rev-parse", "main")], cwd=folder, capture_output=True, text=True,
                             check=False)
        self.assertEqual(ran.returncode, 1, ran.stdout + ran.stderr)
        self.assertIn("style(site): square every corner", ran.stdout)
        self.assertNotIn("Bump", ran.stdout)
        self.assertNotIn("Merge", ran.stdout)
        self.assertNotIn("conventional", ran.stdout)

    def test_a_title_is_held_unless_dependabot_wrote_it(self):
        self.assertEqual(len(check_commits.problems([], "Explain every screen", "someone")), 1)
        self.assertEqual(check_commits.problems([], "chore(deps): Bump undici", "dependabot[bot]"), [])


if __name__ == "__main__":
    unittest.main()
