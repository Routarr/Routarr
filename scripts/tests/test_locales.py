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


SAID = {"HeldBack": "Seconds to wait: {seconds}", "Copied": "{file} copied to {folder}", "Done": "Done"}


def problems(code: str, unread=frozenset()) -> list[str]:
    found = check_locales.calls("api/x.rs", code)
    return check_locales.parameter_problems(found, SAID, set(unread))


class Parameters(unittest.TestCase):
    def test_a_parameter_renamed_in_a_call_is_named_by_file_line_key_and_name(self):
        code = 'fn held() {\n    localizer.translate("HeldBack", &[("wait", &seconds.to_string())]);\n}'
        self.assertEqual(
            problems(code),
            [
                "backend/src/api/x.rs:2: 'HeldBack' says {seconds}, which the call does not pass",
                "backend/src/api/x.rs:2: 'HeldBack' is passed 'wait', which its English does not say",
            ],
        )

    def test_a_chain_is_read_link_by_link_and_a_value_holding_brackets_is_text(self):
        right = 'Detail::new("Copied")\n    .with("file", "a, (b]")\n    .with("folder", &folder)'
        self.assertEqual(problems(right), [])
        short = 'Told::new("Copied").with("file", name)'
        self.assertEqual(
            problems(short), ["backend/src/api/x.rs:1: 'Copied' says {folder}, which the call does not pass"]
        )
        self.assertEqual(problems('translate("Done", &[])'), [])

    def test_a_comment_and_the_definition_are_not_calls(self):
        code = '/// translate("HeldBack", &[])\npub fn translate(&self, key: &str) {}\nfn new() {}'
        self.assertEqual(check_locales.calls("api/x.rs", code), [])

    def test_a_call_the_check_cannot_read_fails_until_it_is_named(self):
        for code in ['translate(key, &[])', 'translate("HeldBack", &said)', 'Detail::new("Copied").with(done, 1)']:
            self.assertEqual(len(problems(code)), 1, code)
            self.assertIn("name the call in UNREAD", problems(code)[0])
        self.assertEqual(problems("translate(key, &[])", {("api/x.rs", "key")}), [])

    def test_an_entry_no_call_needs_fails(self):
        gone = {("api/x.rs", "refusal")}
        self.assertEqual(
            problems('translate("Done", &[])', gone),
            ["UNREAD names refusal in backend/src/api/x.rs, which no call there needs"],
        )
        whole = {("api/x.rs", "Copied")}
        self.assertEqual(len(problems('Told::new("Copied").with("file", f).with("folder", d)', whole)), 1)
        self.assertEqual(problems('Told::new("Copied").with("file", f)', whole), [])


if __name__ == "__main__":
    unittest.main()
