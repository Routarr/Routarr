"""The harnesses that start a throwaway Routarr with a key this repository publishes."""

import re
import unittest

from _scripts import ROOT

HARNESSES = ["frontend/e2e/run.sh", "site/screenshots/run.sh"]


def server_commands(script: str) -> list[str]:
    """Each command that starts the release binary, its continued lines joined."""
    commands = re.sub(r"\\\n", " ", (ROOT / script).read_text()).split("\n")
    return [command for command in commands if re.search(r'/release/routarr"\s*>', command)]


class Harnesses(unittest.TestCase):
    def test_every_harness_server_listens_on_the_loopback_alone(self):
        for script in HARNESSES:
            with self.subTest(script):
                commands = server_commands(script)
                self.assertTrue(commands, f"{script} starts no release binary this test can read")
                for command in commands:
                    self.assertIn("ROUTARR_HOST=127.0.0.1", command, f"{script} binds every interface")


if __name__ == "__main__":
    unittest.main()
