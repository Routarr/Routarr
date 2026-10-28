"""`scripts/check.py`, which runs locally what `ci.yml` runs, read from the workflow itself."""

import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from _scripts import ROOT, SCRIPTS, load

check = load("check")
WORKFLOW = (ROOT / ".github" / "workflows" / "ci.yml").read_text()


class Check(unittest.TestCase):
    def test_every_job_of_the_workflow_has_an_area_or_runs_nothing_to_check(self):
        jobs = WORKFLOW.split("\njobs:\n", 1)[1]
        names = set(re.findall(r"^  ([a-z0-9-]+):\s*$", jobs, re.MULTILINE))
        self.assertEqual(names, set(check.AREAS) | check.CI_ONLY_JOBS)

    def test_every_area_reads_steps_it_can_run(self):
        steps = check.parse(WORKFLOW)
        for area in check.ORDER:
            runnable = [s for s in steps if s.area == area and not s.ci_only()]
            self.assertTrue(runnable, f"{area} reads no step it can run")

    def test_the_steps_keep_their_working_directory(self):
        steps = {s.run: s.directory for s in check.parse(WORKFLOW)}
        self.assertEqual(steps["cargo clippy --locked --all-targets -- -D warnings"], "backend")
        self.assertEqual(steps["python3 scripts/check-locales.py"], ".")
        self.assertEqual(steps["npm run test:e2e"], "frontend")
        self.assertEqual(steps["npx astro check"], "site")

    def test_a_job_environment_reaches_its_steps(self):
        steps = {s.run: s.env for s in check.parse(WORKFLOW)}
        self.assertEqual(steps["npm run test:e2e"].get("ROUTARR_E2E_BUILT"), "1")

    def test_a_step_keeps_its_own_environment(self):
        steps = {s.run: s.env for s in check.parse(WORKFLOW)}
        self.assertEqual(steps["cargo doc --no-deps --locked"].get("RUSTDOCFLAGS"), "-D warnings")

    def test_a_command_keeps_the_quotes_it_ends_with(self):
        runs = [s.run for s in check.parse(WORKFLOW)]
        self.assertTrue(any(run.startswith("python3 scripts/check-commits.py") and run.endswith('"$BASE"')
                            for run in runs), "the commit check lost its closing quote")

    def test_a_step_stops_at_its_first_failure(self):
        ran = subprocess.run([*check.SHELL, "false\ntrue"], check=False)
        self.assertNotEqual(ran.returncode, 0)

    def test_a_branch_with_no_origin_main_checks_every_area(self):
        folder = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, folder)
        environment = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                       "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}

        def git(*arguments):
            subprocess.run(["git", *arguments], cwd=folder, env=environment,
                           capture_output=True, check=True)

        Path(folder, "scripts").mkdir()
        shutil.copy(SCRIPTS / "changed-areas.sh", Path(folder, "scripts"))
        Path(folder, "backend/src").mkdir(parents=True)
        Path(folder, "backend/src/main.rs").write_text("one\n")
        git("init", "-q", "-b", "main")
        git("add", ".")
        git("commit", "-qm", "base")
        git("checkout", "-qb", "feature")
        Path(folder, "backend/src/main.rs").write_text("two\n")
        git("commit", "-qam", "change")
        original = check.ROOT
        check.ROOT = Path(folder)
        self.addCleanup(setattr, check, "ROOT", original)
        self.assertIn("backend", check.changed_areas(None))

    def test_every_step_kept_for_ci_names_a_step_the_workflow_has(self):
        runs = [s.run for s in check.parse(WORKFLOW)]
        for start in check.CI_ONLY_STEPS:
            self.assertTrue(any(run.startswith(start) for run in runs), f"no step starts {start!r}")

    def test_no_step_writing_to_the_tree_is_run(self):
        for step in check.parse(WORKFLOW):
            if step.ci_only():
                continue
            self.assertNotRegex(step.run, r"cargo fmt(?!.*--check)|npm run format(?!:check)|npm ci\b")
            self.assertNotIn("ROUTARR_WRITE_CONTRACT", step.run)

    def test_every_area_the_area_script_names_is_one_it_knows(self):
        script = (SCRIPTS / "changed-areas.sh").read_text()
        printed = set(re.findall(r'^echo "([a-z]+)=', script, re.MULTILINE))
        self.assertTrue(printed)
        self.assertLessEqual(printed, set(check.ORDER) | set(check.DOCKER_AREAS))

    def test_the_heavy_areas_make_room_first_on_the_ramdisk_alone(self):
        on_ramdisk = {"CARGO_TARGET_DIR": "/ramdisk/cargo-target"}
        for area in ("coverage", "e2e"):
            with self.subTest(area=area):
                command = check.room_command(area, on_ramdisk)
                self.assertEqual(command[-2:], ["--room", str(check.ROOM_KB[area])])
        for area in ("repository", "backend", "frontend", "site"):
            with self.subTest(area=area):
                self.assertIsNone(check.room_command(area, on_ramdisk))
        moved = {"CARGO_TARGET_DIR": "/mnt/fast/target", "ROUTARR_RAMDISK": "/mnt/fast"}
        self.assertIsNotNone(check.room_command("e2e", moved))
        for elsewhere in ({}, {"CARGO_TARGET_DIR": "/home/me/target"}, {"CARGO_TARGET_DIR": "/ramdisk-2/target"}):
            with self.subTest(environment=elsewhere):
                self.assertIsNone(check.room_command("coverage", elsewhere))

    def test_the_list_names_each_step_with_its_area_and_directory(self):
        listed = subprocess.run(
            ["python3", str(SCRIPTS / "check.py"), "--list"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout
        self.assertIn("backend\tbackend\tcargo clippy --locked --all-targets -- -D warnings", listed)
        self.assertIn("(CI only:", listed)


if __name__ == "__main__":
    unittest.main()
