"""`.gitignore` and `.dockerignore`, which keep the files Routarr writes out of git and the build."""

import re
import subprocess
import unittest
from pathlib import PurePosixPath

from _scripts import ROOT

# What Routarr writes beside its database, under each data folder a clone can hold.
RUNTIME = [
    "routarr.db",
    "routarr.db-wal",
    "routarr.key",
    "routarr.api_key",
    "routarr.password",
    "routarr.db.pre-restore",
    "routarr.db.pre-restore-wal",
    "routarr.key.pre-restore",
    "routarr.db.restore-pending",
    "routarr.db.restore-pending.json",
    "routarr.key.restore-staging",
    ".backup-work/routarr.db",
    "backups/routarr-backup-20261022-120000.zip",
    "backups/routarr-backup-20261022-120000.zip.enc",
]
FOLDERS = ["config", "data", "backend/data"]


def docker_pattern(pattern: str) -> re.Pattern:
    """A `.dockerignore` line as moby's patternmatcher compiles it."""
    out, at = "^", 0
    while at < len(pattern):
        if pattern.startswith("**", at):
            at += 2
            if pattern.startswith("/", at):
                at += 1
            out += ".*" if at == len(pattern) else "(.*/)?"
        elif pattern[at] == "*":
            out, at = out + "[^/]*", at + 1
        elif pattern[at] == "?":
            out, at = out + "[^/]", at + 1
        else:
            out, at = out + re.escape(pattern[at]), at + 1
    return re.compile(out + "$")


def docker_ignores(path: str) -> bool:
    """Whether the build context leaves `path` out: the last pattern matching it or one of its
    parent folders decides."""
    parents = [str(parent) for parent in reversed(PurePosixPath(path).parents) if str(parent) != "."]
    ignored = False
    for line in (ROOT / ".dockerignore").read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        keep = line.startswith("!")
        pattern = docker_pattern(line.lstrip("!").strip().strip("/"))
        if keep == ignored and any(pattern.match(p) for p in [path, *parents]):
            ignored = not keep
    return ignored


def copied_sources(dockerfile: str) -> list[str]:
    """What a Dockerfile copies from the build context, every `COPY` without `--from`."""
    sources = []
    for line in (ROOT / dockerfile).read_text().splitlines():
        words = line.split()
        if words[:1] == ["COPY"] and not any(word.startswith("--from") for word in words):
            sources += [word for word in words[1:-1] if not word.startswith("--")]
    return sources


class Ignored(unittest.TestCase):
    def test_runtime_files_stay_out_of_git(self):
        for folder in FOLDERS:
            for name in RUNTIME:
                path = f"{folder}/{name}"
                with self.subTest(path):
                    ignored = subprocess.run(
                        ["git", "check-ignore", "-q", "--no-index", path], cwd=ROOT, check=False
                    )
                    self.assertEqual(ignored.returncode, 0, f"git would offer {path} to a commit")

    def test_runtime_files_stay_out_of_the_build_context(self):
        for folder in FOLDERS:
            for name in RUNTIME:
                path = f"{folder}/{name}"
                with self.subTest(path):
                    self.assertTrue(docker_ignores(path), f"the build context carries {path}")
        for secret in ["backend/.env", "frontend/.env"]:
            self.assertTrue(docker_ignores(secret), f"the build context carries {secret}")

    def test_every_file_a_dockerfile_copies_reaches_the_build_context(self):
        for dockerfile in ["Dockerfile", ".devcontainer/Dockerfile"]:
            sources = copied_sources(dockerfile)
            self.assertTrue(sources, f"read no COPY in {dockerfile}")
            for source in sources:
                tracked = subprocess.run(
                    ["git", "ls-files", "--", source], cwd=ROOT, capture_output=True, text=True, check=True
                ).stdout.split()
                self.assertTrue(tracked, f"{dockerfile} copies {source}, which git does not track")
                for path in tracked:
                    with self.subTest(dockerfile=dockerfile, path=path):
                        self.assertFalse(docker_ignores(path), f"{dockerfile} copies {path}, left out")

    def test_the_matcher_reads_an_exception_under_a_folder_left_out(self):
        self.assertTrue(docker_pattern("**/backups").match("config/backups"))
        self.assertFalse(docker_pattern("*").match("backend/src"))
        self.assertTrue(docker_pattern("backend/src").match("backend/src"))


if __name__ == "__main__":
    unittest.main()
