"""`scripts/check-migrations.py`, which holds the shipped migrations and the fold."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from _scripts import SCRIPTS

SHIPPED = "-- The first release.\nCREATE TABLE rules (id TEXT PRIMARY KEY);\n"


def git(folder: str, *arguments: str) -> str:
    environment = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                   "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
    return subprocess.run(["git", *arguments], cwd=folder, env=environment,
                          capture_output=True, text=True, check=True).stdout


class Migrations(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.folder)
        git(self.folder, "init", "-q")
        self.write("001_release_0_1_0.sql", SHIPPED)
        git(self.folder, "add", ".")
        git(self.folder, "commit", "-qm", "release")
        git(self.folder, "tag", "v0.1.0")

    def write(self, name: str, sql: str):
        path = Path(self.folder, "backend/migrations", name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(sql)

    def check(self, *arguments: str) -> subprocess.CompletedProcess:
        git(self.folder, "add", ".")
        git(self.folder, "commit", "-qm", "change", "--allow-empty")
        return subprocess.run(
            ["python3", str(SCRIPTS / "check-migrations.py"), *arguments],
            cwd=self.folder, capture_output=True, text=True, check=False,
        )

    def test_a_shipped_statement_edited_is_refused(self):
        self.write("001_release_0_1_0.sql", SHIPPED.replace("id TEXT", "id INTEGER"))
        refused = self.check()
        self.assertEqual(refused.returncode, 1, refused.stderr)
        self.assertIn("001_release_0_1_0.sql shipped in v0.1.0", refused.stderr)

    def test_a_comment_of_a_shipped_file_may_change(self):
        self.write("001_release_0_1_0.sql", "-- Reworded.\n" + SHIPPED.split("\n", 1)[1])
        self.assertEqual(self.check().returncode, 0)

    def test_a_release_with_two_new_files_is_refused_until_they_fold(self):
        self.write("002_rule_template.sql", "ALTER TABLE rules ADD COLUMN template TEXT;\n")
        self.write("003_release_0_2_0.sql", "CREATE TABLE notes (id TEXT);\n")
        self.assertEqual(self.check().returncode, 0, "a push between releases is refused")
        refused = self.check("--release", "0.2.0")
        self.assertEqual(refused.returncode, 1)
        self.assertIn("002_rule_template.sql, 003_release_0_2_0.sql", refused.stderr)

        Path(self.folder, "backend/migrations/002_rule_template.sql").unlink()
        self.assertEqual(self.check("--release", "0.2.0-rc.1").returncode, 0)


if __name__ == "__main__":
    unittest.main()
