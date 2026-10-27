"""`scripts/check-locales.py` and `scripts/add-locale.py`, the dictionaries' gate."""

import contextlib
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from _scripts import SCRIPTS, load

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

    def test_a_family_that_reads_nothing_and_a_built_key_english_lacks_are_named(self):
        families = {"Job": set(), "Status": {"StatusDone", "StatusGone"}}
        self.assertEqual(
            check_locales.built_problems(families, {"StatusDone": "Done"}),
            [
                "read no Job value from backend/src/jobs/registry.rs",
                "built at run time but missing from en.json: StatusGone",
            ],
        )

    def test_a_stray_key_a_changed_placeholder_an_empty_value_and_a_thin_file_are_named(self):
        dictionaries = {
            "en": ENGLISH,
            "fr": {"Dashboard": "Tableau de bord", "Moved": "Déplacés", "Cancel": " ", "Stray": "x"},
            "de": {"Cancel": "Abbrechen"},
        }
        problems, completion = check_locales.translation_problems(ENGLISH, dictionaries)
        self.assertEqual(
            problems,
            [
                (
                    "de.json is only 33% translated (2 keys missing, minimum 50%). "
                    "A partial translation is fine, but this looks like a broken file."
                ),
                "fr.json has a key English does not: Stray",
                "fr.json: placeholders differ for 'Moved' (['count'] vs [])",
                "fr.json: 'Cancel' is empty",
            ],
        )
        self.assertEqual(completion, {"de": 1 - 2 / 3, "fr": 1.0})

    def test_a_key_no_source_quotes_and_no_family_builds_is_named(self):
        english = {**ENGLISH, "StatusDone": "Done"}
        text = "t('Dashboard') translate(\"Cancel\")"
        self.assertEqual(
            check_locales.orphan_problems(english, text, {"StatusDone"}),
            ["defined but never referenced: Moved"],
        )

    def test_the_first_key_out_of_the_english_order_is_named(self):
        dictionaries = {"en": ENGLISH, "fr": {"Moved": "Déplacés : {count}", "Dashboard": "Tableau"}}
        self.assertEqual(
            check_locales.order_problems(dictionaries, ENGLISH),
            [
                (
                    "fr.json leaves the English key order at 'Moved' (expected 'Dashboard'): "
                    "echo '{}' | python3 scripts/add-locale.py fr"
                ),
            ],
        )

    def test_a_confirmation_label_that_starts_with_the_cancel_word_is_named(self):
        text = "askConfirmation(t('ConfirmStop'), 'StopAll')\nask(t('Which'), [{ label: 'KeepBoth' }])"
        labels = check_locales.confirmation_labels(text)
        self.assertEqual(labels, {"StopAll", "KeepBoth"})
        english = {"Cancel": "Cancel", "StopAll": "Stop all", "KeepBoth": "Keep both"}
        dictionaries = {"en": english, "fr": {"Cancel": "Annuler", "StopAll": "Annuler tout"}}
        self.assertEqual(
            check_locales.cancel_problems(dictionaries, english, labels),
            [
                (
                    "fr.json: 'StopAll' ('Annuler tout') starts with the Cancel word 'annuler', "
                    "and the two sit side by side in a confirmation"
                ),
            ],
        )

    def test_each_placeholder_is_a_count_or_not_once_and_each_listed_one_is_used(self):
        english = {"A": "Moved: {count}", "B": "{title} and {weird}"}
        self.assertEqual(
            check_locales.count_problems(english, {"count", "title", "gone"}, {"title", "spare"}),
            [
                (
                    "placeholder {weird} is neither in COUNTS (backend/src/localization.rs) "
                    "nor in NOT_COUNTS (scripts/check-locales.py)"
                ),
                "placeholder {title} is both a count and not one",
                "placeholder {gone} is classed but no sentence uses it",
                "placeholder {spare} is classed but no sentence uses it",
            ],
        )
        self.assertEqual(
            check_locales.count_problems({}, set(), set()),
            ["read no COUNTS from backend/src/localization.rs"],
        )

    def test_a_plain_space_where_french_puts_a_no_break_one_is_named(self):
        french = {"Ready": "Prêt ?", "Fine": "Prêt\u00a0?", "Quoted": "« x\u00a0»"}
        self.assertEqual(
            check_locales.spacing_problems(french),
            [
                "fr.json: 'Ready' has a plain space in ' ?', where French puts a no-break space (U+00A0)",
                "fr.json: 'Quoted' has a plain space in '« ', where French puts a no-break space (U+00A0)",
            ],
        )

    def test_the_made_up_keys_of_a_trailing_test_module_are_not_read(self):
        code = 'fn real() { translate("Kept", &[]) }\n#[cfg(test)]\nmod tests {\n    translate("MadeUp", &[]);\n}\n'
        stripped = check_locales.strip_rust_tests(code)
        self.assertIn('"Kept"', stripped)
        self.assertNotIn("MadeUp", stripped)

    def test_a_key_written_twice_is_named(self):
        with tempfile.TemporaryDirectory() as folder:
            path = pathlib.Path(folder) / "fr.json"
            path.write_text('{"Cancel": "Annuler", "Moved": "x", "Cancel": "Annuler (bis)"}', encoding="utf-8")
            problems: list[str] = []
            self.assertEqual(
                check_locales.load_dictionary(path, problems), {"Cancel": "Annuler (bis)", "Moved": "x"}
            )
        self.assertEqual(problems, ["fr.json: 'Cancel' is written twice"])


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


def missing(text: str, english=ENGLISH, ids=frozenset(), not_keys=frozenset()) -> list[str]:
    called = {call.key for call in check_locales.calls("api/x.rs", text) if call.key}
    return check_locales.missing_keys(text, english, called, set(ids), set(not_keys))


class Keys(unittest.TestCase):
    def test_a_key_english_lacks_is_named_whatever_call_or_field_holds_it(self):
        code = (
            'Told::new("ArrBrandNewKey")\nDetail::new("JobDetailBrandNew")\n'
            'Card { text: "HelpProblemBrandNew" }\nTold::new("Frobnicate")\nt(\'Dashboard\')'
        )
        aside = " (a literal naming no sentence goes in NOT_KEYS)"
        self.assertEqual(
            missing(code),
            [
                f"referenced but missing from en.json: ArrBrandNewKey{aside}",
                "referenced but missing from en.json: Frobnicate",
                f"referenced but missing from en.json: HelpProblemBrandNew{aside}",
                f"referenced but missing from en.json: JobDetailBrandNew{aside}",
            ],
        )

    def test_a_literal_naming_no_sentence_and_a_help_id_are_not_keys(self):
        code = "this.name = 'ApiError'\ncards: ['DashboardInstances']"
        self.assertEqual(missing(code, ids={"DashboardInstances"}, not_keys={"ApiError"}), [])

    def test_a_not_keys_entry_that_is_a_key_or_that_no_source_quotes_is_named(self):
        english = {"MovedTitles": "Moved titles"}
        self.assertEqual(
            missing("t('MovedTitles')", english, not_keys={"MovedTitles", "GoneClient"}),
            [
                "NOT_KEYS names MovedTitles, which en.json has as a key",
                "NOT_KEYS names GoneClient, which no source quotes",
            ],
        )


PIN = {"en": r"\bpin(s|ned)?\b", "except": [], "words": {"fr": "épingl"}}
APPLY = {"en": r"\bapply\b", "except": [], "words": {"fr": "appliqu"}}


def glossary(english: dict[str, str], french: dict[str, str], **concepts) -> list[str]:
    return check_locales.glossary_problems(english, {"en": english, "fr": french}, concepts)


class Glossary(unittest.TestCase):
    def test_a_key_saying_the_concept_in_another_word_is_named(self):
        self.assertEqual(
            glossary({"Pin": "Pin a title"}, {"Pin": "Fixer un titre"}, pin=PIN),
            ["fr.json: 'Pin' says 'pin' in another word than /épingl/"],
        )
        self.assertEqual(glossary({"Pin": "Pin a title"}, {"Pin": "Épingler un titre"}, pin=PIN), [])

    def test_an_exception_whose_english_does_not_say_the_word_is_named(self):
        self.assertEqual(
            glossary({"Pin": "Pin it", "Other": "Keep it"}, {}, pin={**PIN, "except": ["Other"]}),
            ["glossary: 'pin' excepts Other, whose English does not say it"],
        )

    def test_a_homonym_is_read_without_case_unless_the_concept_keeps_it(self):
        english = {"Pin": "Pin it", "Forced": "Force it"}
        french = {"Pin": "Épingler", "Forced": "Épingler de force"}
        self.assertEqual(
            glossary(english, french, pin={**PIN, "homonym": r"\bforce\b"}),
            [
                (
                    "fr.json: 'Forced' uses the 'pin' word /épingl/ for what its English says as "
                    "/\\bforce\\b/"
                ),
            ],
        )
        english = {"Apply": "Apply", "Ended": "{arr} ended it"}
        french = {"Apply": "Appliquer", "Ended": "{arr} l'a appliqué"}
        held = {**APPLY, "homonym": r"\bArr\b", "homonym_case": True}
        self.assertEqual(glossary(english, french, apply=held), [])
        self.assertEqual(len(glossary(english, french, apply={**held, "homonym_case": False})), 1)

    def test_the_word_beside_the_placeholder_naming_that_thing_is_named(self):
        english = {"Apply": "Apply", "Answers": "{service} answers"}
        french = {"Apply": "Appliquer", "Answers": "Appliquer {service} répond"}
        concept = {**APPLY, "beside": r"\{service\}"}
        self.assertEqual(
            glossary(english, french, apply=concept),
            ["fr.json: 'Answers' names {service} with the 'apply' word /appliqu/"],
        )
        self.assertEqual(glossary(english, french, apply={**concept, "beside_skip": ["fr"]}), [])


class Merge(unittest.TestCase):
    def test_the_merge_runs_as_a_command(self):
        run = subprocess.run(
            [sys.executable, str(SCRIPTS / "add-locale.py"), "fr"],
            input="[]", capture_output=True, text=True, check=False,
        )
        self.assertEqual(run.returncode, 1, run.stderr)
        self.assertIn("the batch is not a JSON object of keys and translations", run.stderr)

    def test_a_file_holding_a_key_twice_is_not_rewritten(self):
        with tempfile.TemporaryDirectory() as folder:
            locales = pathlib.Path(folder)
            (locales / "en.json").write_text(json.dumps(ENGLISH), encoding="utf-8")
            twice = '{"Dashboard": "Tableau", "Dashboard": "Tableau de bord"}'
            (locales / "fr.json").write_text(twice, encoding="utf-8")
            stderr = io.StringIO()
            with (
                mock.patch.object(add_locale, "LOCALES", locales),
                mock.patch.object(sys, "argv", ["add-locale.py", "fr"]),
                mock.patch.object(sys, "stdin", io.StringIO('{"Cancel": "Annuler"}')),
                contextlib.redirect_stderr(stderr),
            ):
                self.assertEqual(add_locale.main(), 1)
            self.assertEqual((locales / "fr.json").read_text(encoding="utf-8"), twice)
        self.assertIn("fr.json: 'Dashboard' is written twice", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
