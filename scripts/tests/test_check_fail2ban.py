"""`scripts/check-fail2ban.py`, the fail2ban recipe of `SECURITY.md` against Routarr's lines."""

import unittest

from _scripts import ROOT, load

check_fail2ban = load("check-fail2ban")

SAMPLE = (
    '2026-10-06T23:04:23.117345918+02:00  WARN request{method=POST uri=/api/v1/auth/login '
    'id="x"}: routarr::audit: A sign-in was refused: wrong name or password. '
    'event="sign_in" outcome="refused" subject="-" client=203.0.113.9'
)
COMPOSE = "services:\n  routarr:\n    container_name: routarr\n"


def document(filter_line: str, jail: str) -> str:
    return f"```text\n{SAMPLE}\n```\n\n```ini\n[Definition]\n{filter_line}\n\n[routarr]\n{jail}\n```\n"


class Recipe(unittest.TestCase):
    def test_the_recipe_shipped_reads_routarr_s_lines(self):
        security = (ROOT / "SECURITY.md").read_text(encoding="utf-8")
        compose = (ROOT / "docker-compose.yml").read_text(encoding="utf-8")
        self.assertEqual(check_fail2ban.problems(security, compose), [])

    def test_a_filter_blind_to_the_quotes_json_file_writes_is_named(self):
        text = document(
            'failregex = routarr::audit: .* outcome="refused" .* client=<HOST>',
            "journalmatch = CONTAINER_NAME=routarr",
        )
        problems = check_fail2ban.problems(text, COMPOSE)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("json-file", problems[0])

    def test_a_filter_banning_on_an_allowed_event_is_named(self):
        text = document(
            "failregex = routarr::audit: .* client=<HOST>", "journalmatch = CONTAINER_NAME=routarr"
        )
        problems = check_fail2ban.problems(text, COMPOSE)
        self.assertTrue(any("allowed" in problem for problem in problems), problems)

    def test_a_jail_reading_every_container_or_another_one_is_named(self):
        text = document(
            'failregex = routarr::audit: .* outcome=\\\\?"refused\\\\?" .* client=<HOST>',
            "logpath = /var/lib/docker/containers/*/*-json.log\njournalmatch = CONTAINER_NAME=sonarr",
        )
        problems = check_fail2ban.problems(text, COMPOSE)
        self.assertEqual(len(problems), 2, problems)


if __name__ == "__main__":
    unittest.main()
