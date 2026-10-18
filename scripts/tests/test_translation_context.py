"""`scripts/translation-context.py`, the brief a translator works from."""

import unittest

from _scripts import load

context = load("translation-context")

ENGLISH = {"Moved": "Moved: {count}", "Kept": "Kept here", "New": "Brand new"}


class Current(unittest.TestCase):
    def test_a_key_the_file_translates_is_shown_as_it_reads_now(self):
        translated = {"Moved": "Déplacés : {count}", "Kept": "Kept here"}
        batch = {"Moved": "Moved now: {count}", "Kept": "Kept here", "New": "Brand new"}
        self.assertEqual(
            context.current(batch, ENGLISH, translated), ["- `Moved`: Déplacés : {count}"]
        )


if __name__ == "__main__":
    unittest.main()
