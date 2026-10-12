"""`scripts/check-licences.py`, the allow-list for what the image ships besides the crates."""

import copy
import importlib.util
import json
import pathlib
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
spec = importlib.util.spec_from_file_location(
    "check_licences", ROOT / "scripts" / "check-licences.py"
)
check_licences = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_licences)

ALLOWED = tomllib.loads((ROOT / "backend/deny.toml").read_text())["licenses"]["allow"]
LOCK = json.loads((ROOT / "frontend/package-lock.json").read_text())
ABOUT = tomllib.loads((ROOT / "backend/about.toml").read_text())


def with_licence(path, licence):
    lock = copy.deepcopy(LOCK)
    lock["packages"][path]["license"] = licence
    return lock


class CheckLicences(unittest.TestCase):
    def test_the_tree_keeps_to_the_allow_list(self):
        self.assertEqual(check_licences.npm_refusals(LOCK, set(ALLOWED)), [])
        self.assertEqual(check_licences.about_refusals(ABOUT, ALLOWED), [])

    def test_a_bundled_package_under_another_licence_is_refused(self):
        refusals = check_licences.npm_refusals(
            with_licence("node_modules/clsx", "SSPL-1.0"), set(ALLOWED)
        )
        self.assertEqual(refusals, ["clsx is SSPL-1.0, which deny.toml does not allow"])

    def test_a_bundled_package_declaring_no_licence_is_refused(self):
        lock = copy.deepcopy(LOCK)
        del lock["packages"]["node_modules/svelte"]["license"]
        self.assertEqual(
            check_licences.npm_refusals(lock, set(ALLOWED)), ["svelte declares no licence"]
        )

    def test_a_development_tool_is_not_shipped_so_not_held(self):
        dev = next(
            path for path, entry in LOCK["packages"].items() if path and entry.get("dev")
        )
        self.assertEqual(
            check_licences.npm_refusals(with_licence(dev, "SSPL-1.0"), set(ALLOWED)), []
        )

    def test_an_expression_needs_one_side_of_or_and_both_of_and(self):
        allowed = {"MIT", "Apache-2.0 WITH LLVM-exception"}
        self.assertTrue(check_licences.allows("(MIT OR SSPL-1.0)", allowed))
        self.assertFalse(check_licences.allows("MIT AND SSPL-1.0", allowed))
        self.assertTrue(check_licences.allows("Apache-2.0 WITH LLVM-exception OR GPL-2.0", allowed))
        self.assertFalse(check_licences.allows("Apache-2.0", allowed))
        self.assertTrue(check_licences.allows("SSPL-1.0 AND (BSD-3-Clause OR MIT) OR MIT", allowed))
        with self.assertRaises(ValueError):
            check_licences.allows("(MIT OR", allowed)

    def test_about_toml_accepts_exactly_what_deny_toml_allows(self):
        fewer = {"accepted": [licence for licence in ALLOWED if licence != "Zlib"]}
        self.assertEqual(
            check_licences.about_refusals(fewer, ALLOWED),
            ["about.toml does not accept Zlib, which deny.toml allows"],
        )
        more = {"accepted": [*ALLOWED, "SSPL-1.0"]}
        self.assertEqual(
            check_licences.about_refusals(more, ALLOWED),
            ["about.toml accepts SSPL-1.0, which deny.toml does not allow"],
        )


if __name__ == "__main__":
    unittest.main()
