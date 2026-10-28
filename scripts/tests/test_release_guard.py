"""`scripts/release-guard.sh` against a stand-in for `gh` that answers GitHub's API as GitHub does."""

import os
import pathlib
import subprocess
import tempfile
import textwrap
import unittest

from _scripts import SCRIPTS

SHA = "f01dce6fa88ddc679fa2de80ec074341c135dd3f"

# GitHub resolves `commits/refs/tags/<tag>` to the tagged commit, and answers 422 to
# `commits/tags/<tag>`, which names no ref.
FAKE_GH = textwrap.dedent(f"""\
    #!/usr/bin/env bash
    case "$2" in
      repos/Routarr/Routarr/commits/refs/tags/v0.3.0-rc.1) echo {SHA} ;;
      *) echo "No commit found for SHA: $2 (HTTP 422)" >&2; exit 1 ;;
    esac
    """)


def guard(mode):
    with tempfile.TemporaryDirectory() as folder:
        gh = pathlib.Path(folder, "gh")
        gh.write_text(FAKE_GH)
        gh.chmod(0o755)
        env = dict(os.environ, PATH=f"{folder}:{os.environ['PATH']}", GITHUB_RUN_ATTEMPT="2")
        return subprocess.run(
            ["bash", str(SCRIPTS / "release-guard.sh"), mode, "Routarr/Routarr", SHA, "v0.3.0-rc.1"],
            env=env, capture_output=True, text=True, check=False,
        )


class StillTagged(unittest.TestCase):
    def test_a_run_publishing_its_tag_finds_the_tag_where_it_was(self):
        result = guard("--publishing")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("still names", result.stdout)

    def test_a_published_release_finds_its_tag_before_the_followed_tags_move(self):
        result = guard("--promoting")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("may move onto 0.3.0-rc.1", result.stdout)


if __name__ == "__main__":
    unittest.main()
