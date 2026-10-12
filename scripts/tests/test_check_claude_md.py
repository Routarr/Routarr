"""`scripts/check-claude-md.py`, the globs that load each rule."""

import unittest

from _scripts import load

check_claude_md = load("check-claude-md")

FILES = [
    "backend/src/main.rs",
    "backend/Cargo.toml",
    "scripts/check-locales.py",
    "scripts/glossary.json",
    "frontend/e2e/run.sh",
    "docker-compose.yml",
    "README.md",
]


def matches(glob: str) -> list[str]:
    pattern = check_claude_md.glob_pattern(glob)
    return [path for path in FILES if pattern.match(path)]


class Globs(unittest.TestCase):
    def test_a_double_star_crosses_folders_and_a_star_does_not(self):
        self.assertEqual(matches("backend/**"), ["backend/src/main.rs", "backend/Cargo.toml"])
        self.assertEqual(matches("backend/*"), ["backend/Cargo.toml"])
        self.assertEqual(matches("**/*.sh"), ["frontend/e2e/run.sh"])
        self.assertEqual(matches("docker-compose*.yml"), ["docker-compose.yml"])

    def test_braces_name_alternatives(self):
        self.assertEqual(
            matches("scripts/{check-locales.py,glossary.json}"),
            ["scripts/check-locales.py", "scripts/glossary.json"],
        )

    def test_a_glob_that_matches_no_file_is_named(self):
        rule = '---\npaths:\n  - "backend/**"\n  - "fronted/**"\n---\n# A rule\n'
        self.assertEqual(check_claude_md.unmatched_globs(rule, FILES), ["fronted/**"])


if __name__ == "__main__":
    unittest.main()
