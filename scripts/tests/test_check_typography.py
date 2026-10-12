"""`scripts/check-typography.py`, plain punctuation in what a reader sees.

The signs are written as escapes: this file is read by the check too.
"""

import unittest

from _scripts import load

check_typography = load("check-typography")

EM_DASH = "\u2014"
CURLY_QUOTE = "\u201c"
GREEK_RAISED_DOT = "\u0387"
MIDDLE_DOT = "\u00b7"


class Markdown(unittest.TestCase):
    def test_a_semicolon_in_prose_is_named_with_its_line(self):
        problems = check_typography.markdown_problems("A title\n\nOne clause; another.\n")
        self.assertEqual(problems, [(3, "a semicolon")])

    def test_a_semicolon_in_code_is_the_code_s_own(self):
        text = "Run `a; b` first.\n\n```bash\ncd backend; cargo test\n```\n"
        self.assertEqual(check_typography.markdown_problems(text), [])

    def test_an_em_dash_or_a_curly_quote_is_named_even_in_code(self):
        text = f"```\nx {EM_DASH} y\n```\nSay {CURLY_QUOTE}hi.\n"
        self.assertEqual(
            check_typography.markdown_problems(text), [(2, "an em dash"), (4, "a curly quote")]
        )


class Dictionaries(unittest.TestCase):
    def test_greek_writes_its_question_mark_as_a_semicolon(self):
        self.assertEqual(check_typography.value_problems("el", "Συνέχεια;"), [])
        self.assertEqual(check_typography.value_problems("fr", "Continuer ; ou pas"), ["a semicolon"])

    def test_greek_s_own_semicolon_is_refused_in_both_writings(self):
        for value in [f"Ναι{GREEK_RAISED_DOT} όχι", f"Ναι{MIDDLE_DOT}όχι"]:
            self.assertEqual(check_typography.value_problems("el", value), ["a Greek semicolon"])
        self.assertEqual(check_typography.value_problems("el", f"Α {MIDDLE_DOT} Β"), [])

    def test_catalan_writes_a_middle_dot_inside_a_word(self):
        self.assertEqual(check_typography.value_problems("ca", f"col{MIDDLE_DOT}lecció"), [])


if __name__ == "__main__":
    unittest.main()
