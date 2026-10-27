"""`scripts/translation-context.py`, the brief a translator works from."""

import pathlib
import subprocess
import sys
import tempfile
import unittest

from _scripts import SCRIPTS, load

context = load("translation-context")

ENGLISH = {"Moved": "Moved: {count}", "Kept": "Kept here", "New": "Brand new"}


class Current(unittest.TestCase):
    def test_a_key_the_file_translates_is_shown_as_it_reads_now(self):
        translated = {"Moved": "Déplacés : {count}", "Kept": "Kept here"}
        batch = {"Moved": "Moved now: {count}", "Kept": "Kept here", "New": "Brand new"}
        self.assertEqual(
            context.current(batch, ENGLISH, translated), ["- `Moved`: Déplacés : {count}"]
        )


class Habits(unittest.TestCase):
    def test_the_no_break_spaces_a_file_writes_are_named(self):
        notes = context.habits({"Ready": "Prêt\u00a0?", "Quoted": "«\u00a0x\u00a0»"})
        self.assertEqual(
            notes,
            [
                "Quotes are « », with a no-break space inside each.",
                "A no-break space (U+00A0) goes before : ? and !.",
            ],
        )
        self.assertEqual(context.habits({"Ready": "Ready?", "Quoted": "« x »"}), ["Quotes are « »."])


class Batch(unittest.TestCase):
    def test_a_batch_that_is_not_an_object_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            batch = pathlib.Path(folder) / "batch.json"
            batch.write_text('["Cancel"]', encoding="utf-8")
            run = subprocess.run(
                [sys.executable, str(SCRIPTS / "translation-context.py"), str(batch), "fr"],
                capture_output=True, text=True, check=False,
            )
        self.assertEqual(run.returncode, 2, run.stderr)
        self.assertEqual(run.stderr, "the batch is not a JSON object of keys and their English\n")


if __name__ == "__main__":
    unittest.main()
