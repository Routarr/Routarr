"""`.devcontainer/prune.sh`, which bounds the dev container's caches and must delete nothing else."""

import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

from _scripts import ROOT

SHM = Path("/dev/shm")


def tmpfs(path: Path) -> bool:
    found = subprocess.run(["findmnt", "-no", "FSTYPE", "--target", str(path)],
                           capture_output=True, text=True, check=False)
    return found.stdout.strip() == "tmpfs"


class Prune(unittest.TestCase):
    def setUp(self):
        # Copied without `backend/Cargo.toml` beside it, so the pass finds no checkout to trim.
        self.folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.folder)
        (self.folder / ".devcontainer").mkdir()
        shutil.copy(ROOT / ".devcontainer" / "prune.sh", self.folder / ".devcontainer")
        self.environment = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.folder),
            "ROUTARR_VSCODE_CACHE": str(self.folder / "vscode-cache"),
            "ROUTARR_RAMDISK": str(self.folder / "empty-ramdisk"),
        }

    def prune(self, **environment: str) -> str:
        return subprocess.run(
            ["bash", str(self.folder / ".devcontainer" / "prune.sh")],
            env={**self.environment, **environment}, capture_output=True, text=True, check=True,
        ).stdout

    @unittest.skipUnless(tmpfs(SHM), "the sweep acts on a tmpfs only, and /dev/shm is none here")
    def test_the_scratch_sweep_spares_a_checkout_and_takes_a_stale_run(self):
        ramdisk = Path(tempfile.mkdtemp(dir=SHM))
        self.addCleanup(shutil.rmtree, ramdisk)
        worktree = ramdisk / "routarr-wt"
        (worktree / "src").mkdir(parents=True)
        (worktree / ".git").write_text("gitdir: /workspaces/Routarr/.git/worktrees/routarr-wt\n")
        (worktree / "src" / "lib.rs").write_text("edited a moment ago\n")
        for name in ["routarr-e2e-abc123", "routarr-tests-1"]:
            (ramdisk / name).mkdir()
        old = time.time() - 13 * 3600
        for folder in [worktree, ramdisk / "routarr-e2e-abc123", ramdisk / "routarr-tests-1"]:
            os.utime(folder, (old, old))
        self.prune(ROUTARR_RAMDISK=str(ramdisk))
        self.assertTrue((worktree / "src" / "lib.rs").exists())
        self.assertFalse((ramdisk / "routarr-e2e-abc123").exists())
        self.assertFalse((ramdisk / "routarr-tests-1").exists())

    def shared_tree(self, *artefacts: str) -> Path:
        target = self.folder / "shared-target"
        for path in ["debug/deps/other-project-lib.rlib", "debug/incremental/x", "release/x", *artefacts]:
            (target / path).parent.mkdir(parents=True, exist_ok=True)
            (target / path).write_text("built\n")
        return target

    def test_a_build_tree_with_no_routarr_build_is_left_alone(self):
        target = self.shared_tree()
        said = self.prune(CARGO_TARGET_DIR=str(target), ROUTARR_BUILD_BUDGET_KB="1")
        self.assertTrue((target / "debug" / "deps" / "other-project-lib.rlib").exists())
        self.assertTrue((target / "debug" / "incremental").exists())
        self.assertIn("not Routarr's", said)

    def test_a_routarr_build_tree_over_its_budget_is_trimmed(self):
        target = self.shared_tree("debug/routarr")
        self.prune(CARGO_TARGET_DIR=str(target), ROUTARR_BUILD_BUDGET_KB="1")
        self.assertFalse((target / "debug").exists())
        self.assertTrue((target / "release").exists())


if __name__ == "__main__":
    unittest.main()
