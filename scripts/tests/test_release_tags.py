"""`scripts/release-tags.py`, the tags people follow that a published release moves."""

import unittest

from _scripts import load

release_tags = load("release-tags")


def moving(tag, *published):
    return release_tags.moving(tag, list(published))


class ReleaseTags(unittest.TestCase):
    def test_the_newest_release_takes_its_minor_and_latest(self):
        self.assertEqual(moving("v0.3.0", "v0.2.0", "v0.3.0"), ["0.3", "latest"])

    def test_a_patch_of_an_older_minor_leaves_latest_on_the_newest(self):
        self.assertEqual(moving("v0.2.1", "v0.2.0", "v0.3.0", "v0.2.1"), ["0.2"])

    def test_an_older_patch_published_late_moves_nothing(self):
        self.assertEqual(moving("v0.2.1", "v0.2.2", "v0.2.1"), [])

    def test_versions_compare_as_numbers(self):
        self.assertEqual(moving("v0.10.0", "v0.9.3", "v0.10.0"), ["0.10", "latest"])

    def test_a_major_tag_follows_from_one_on(self):
        self.assertEqual(moving("v1.2.0", "v0.9.0", "v1.2.0"), ["1.2", "1", "latest"])
        self.assertEqual(moving("v1.1.4", "v1.2.0", "v1.1.4"), ["1.1"])

    def test_a_pre_release_moves_nothing(self):
        self.assertEqual(moving("v1.0.0-rc.1", "v0.9.0"), [])

    def test_the_release_counts_before_the_list_shows_it(self):
        self.assertEqual(moving("v0.3.0", "v0.2.0"), ["0.3", "latest"])


if __name__ == "__main__":
    unittest.main()
