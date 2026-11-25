"""`scripts/repository-releases.py`, the releases the apt and dnf repositories carry."""

import unittest

from _scripts import load

repository_releases = load("repository-releases")


def kept(*published):
    return repository_releases.kept(list(published))


class RepositoryReleases(unittest.TestCase):
    def test_the_newest_line_keeps_its_three_newest_releases(self):
        self.assertEqual(
            kept("v0.3.0", "v0.3.1", "v0.3.2", "v0.3.3"), ["v0.3.1", "v0.3.2", "v0.3.3"]
        )

    def test_an_older_line_keeps_its_newest_release_alone(self):
        self.assertEqual(
            kept("v0.3.0", "v0.3.1", "v0.4.0", "v0.2.9", "v0.2.10"),
            ["v0.2.10", "v0.3.1", "v0.4.0"],
        )

    def test_versions_compare_as_numbers(self):
        self.assertEqual(kept("v0.9.0", "v0.10.0"), ["v0.9.0", "v0.10.0"])

    def test_a_tag_naming_no_release_is_left_out(self):
        self.assertEqual(kept("v0.3.1", "v0.4.0-rc.1", "nightly", ""), ["v0.3.1"])
        self.assertEqual(kept(), [])


if __name__ == "__main__":
    unittest.main()
