"""`scripts/changed-areas.sh`, which jobs a commit wakes."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from _scripts import SCRIPTS

FILES = [
    "backend/src/main.rs",
    "backend/locales/en.json",
    "backend/rust-toolchain.toml",
    "frontend/src/App.svelte",
    "site/src/pages/index.astro",
    ".devcontainer/Dockerfile",
    "README.md",
    "Dockerfile",
]


def git(folder: str, *arguments: str) -> str:
    environment = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                   "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
    return subprocess.run(["git", *arguments], cwd=folder, env=environment,
                          capture_output=True, text=True, check=True).stdout


class ChangedAreas(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.folder)
        git(self.folder, "init", "-q")
        for name in FILES:
            path = Path(self.folder, name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("one\n")
        git(self.folder, "add", ".")
        git(self.folder, "commit", "-qm", "base")
        self.base = git(self.folder, "rev-parse", "HEAD").strip()

    def areas(self, *changed: str, base: str | None = None) -> dict[str, bool]:
        for name in changed:
            Path(self.folder, name).write_text("two\n")
        git(self.folder, "commit", "-qam", "change")
        answer = subprocess.run(
            ["bash", str(SCRIPTS / "changed-areas.sh"), base or self.base, "HEAD"],
            cwd=self.folder, capture_output=True, text=True, check=True,
        ).stdout
        return {key: value == "true" for key, value in
                (line.split("=") for line in answer.split())}

    def woken(self, *changed: str) -> list[str]:
        return sorted(area for area, wakes in self.areas(*changed).items() if wakes)

    def test_the_route_table_wakes_the_frontend_whose_tests_read_it(self):
        self.assertEqual(self.woken("backend/src/main.rs"), ["backend", "frontend", "image"])

    def test_the_english_dictionary_wakes_every_job_that_reads_it(self):
        self.assertEqual(
            self.woken("backend/locales/en.json"), ["backend", "frontend", "image", "site"]
        )

    def test_a_site_change_costs_neither_the_rust_jobs_nor_the_image(self):
        self.assertEqual(self.woken("site/src/pages/index.astro"), ["site"])

    def test_the_dev_container_wakes_its_own_build_alone(self):
        self.assertEqual(self.woken(".devcontainer/Dockerfile"), ["devcontainer"])

    def test_the_toolchain_wakes_both_images_and_the_backend(self):
        self.assertEqual(
            self.woken("backend/rust-toolchain.toml"), ["backend", "devcontainer", "image"]
        )

    def test_a_base_that_cannot_be_read_wakes_everything(self):
        areas = self.areas("README.md", base="0000000000000000000000000000000000000000")
        self.assertTrue(all(areas.values()), areas)


if __name__ == "__main__":
    unittest.main()
