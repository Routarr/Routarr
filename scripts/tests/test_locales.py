"""`scripts/check-locales.py` and `scripts/add-locale.py`, the dictionaries' gate."""

import unittest

from _scripts import load

check_locales = load("check-locales")
add_locale = load("add-locale")

ENGLISH = {"Dashboard": "Dashboard", "Moved": "Moved: {count}", "Cancel": "Cancel"}


class Batch(unittest.TestCase):
    def test_a_faithful_batch_merges(self):
        batch = {"Dashboard": "Tableau de bord", "Moved": "Déplacés : {count}"}
        self.assertEqual(add_locale.batch_problems(batch, ENGLISH), [])

    def test_a_value_that_is_not_text_is_refused(self):
        batch = {"Dashboard": None, "Cancel": 42, "Moved": ["x"]}
        self.assertEqual(
            add_locale.batch_problems(batch, ENGLISH),
            ["not text: Dashboard", "not text: Cancel", "not text: Moved"],
        )

    def test_a_lost_placeholder_an_unknown_key_and_an_empty_value_are_refused(self):
        batch = {"Moved": "Déplacés", "Nope": "x", "Cancel": "  "}
        self.assertEqual(
            add_locale.batch_problems(batch, ENGLISH),
            [
                "placeholders differ for 'Moved': ['count'] vs []",
                "unknown key: Nope",
                "empty translation: Cancel",
            ],
        )

    def test_a_batch_that_is_not_an_object_is_refused(self):
        self.assertEqual(
            add_locale.batch_problems([], ENGLISH),
            ["the batch is not a JSON object of keys and translations"],
        )

    def test_both_scripts_read_a_placeholder_with_one_function(self):
        self.assertTrue(add_locale.placeholders.__code__.co_filename.endswith("check-locales.py"))


class Check(unittest.TestCase):
    def test_a_value_that_is_not_text_is_named_and_left_out(self):
        dictionaries = {"en": dict(ENGLISH), "fr": {"Dashboard": None, "Cancel": "Annuler"}}
        problems = check_locales.not_text(dictionaries)
        self.assertEqual(problems, ["fr.json: 'Dashboard' is not a string"])
        self.assertEqual(dictionaries["fr"], {"Cancel": "Annuler"})

    def test_a_dictionary_that_is_not_an_object_is_named(self):
        dictionaries = {"en": dict(ENGLISH), "de": ["Abbrechen"]}
        self.assertEqual(check_locales.not_text(dictionaries), ["de.json is not a JSON object"])
        self.assertEqual(dictionaries["de"], {})

    def test_every_family_of_keys_built_at_run_time_reads_a_key(self):
        for prefix, keys in check_locales.built_keys().items():
            self.assertTrue(keys, f"no {prefix} key read from {check_locales.BUILT[prefix][0]}")


if __name__ == "__main__":
    unittest.main()
