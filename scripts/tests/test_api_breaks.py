"""`scripts/oasdiff-levels.txt`, the oasdiff findings `check-api-breaks.sh` fails on."""

import copy
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from _scripts import ROOT, SCRIPTS

CONTRACT = json.loads((ROOT / "backend" / "openapi" / "v1.json").read_text())


@unittest.skipUnless(shutil.which("oasdiff"), "oasdiff is not installed (ci.yml installs it first)")
class ApiBreaks(unittest.TestCase):
    def breaks(self, changed: dict) -> int:
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        (folder / "base.json").write_text(json.dumps(CONTRACT))
        (folder / "changed.json").write_text(json.dumps(changed))
        return subprocess.run(
            ["oasdiff", "breaking", str(folder / "base.json"), str(folder / "changed.json"),
             "--severity-levels", str(SCRIPTS / "oasdiff-levels.txt"), "--fail-on", "ERR",
             "--flatten-allof"],
            capture_output=True, check=False,
        ).returncode

    def test_a_request_field_removed_is_a_break(self):
        changed = copy.deepcopy(CONTRACT)
        del changed["components"]["schemas"]["CreateCategoryRequest"]["properties"]["description"]
        self.assertEqual(self.breaks(changed), 1)

    def test_a_query_parameter_removed_is_a_break(self):
        changed = copy.deepcopy(CONTRACT)
        operation = changed["paths"]["/library"]["get"]
        operation["parameters"] = [p for p in operation["parameters"] if p.get("name") != "search"]
        self.assertEqual(self.breaks(changed), 1)

    def test_the_contract_breaks_nothing_against_itself(self):
        self.assertEqual(self.breaks(CONTRACT), 0)


if __name__ == "__main__":
    unittest.main()
