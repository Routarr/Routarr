#!/usr/bin/env python3
"""Hold the fail2ban recipe of `SECURITY.md` to the lines Routarr writes.

The filter's `failregex`, its `<HOST>` expanded and run with `re` as fail2ban does, must find
the sample refusal's client in the journald line and in the `json-file` one, quotes written
`\\"`, and nothing once the event is allowed. A file jail reads one container's log, never a
glob over all of them, and a journal jail matches the container `docker-compose.yml` names. The
backend test `tests::audit` holds the sample line to the text log.

Run from anywhere: the paths resolve from this file. Exits non-zero naming each fault.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SECURITY = ROOT / "SECURITY.md"
COMPOSE = ROOT / "docker-compose.yml"
SAMPLE_CLIENT = "203.0.113.9"

# fail2ban's own `<HOST>` (`server/failregex.py`): IPv4, IPv6, either bracketed, or a name.
IP4 = r"(?:\d{1,2}|1\d{2}|2[0-4]\d|25[0-5])(?:\.(?:\d{1,2}|1\d{2}|2[0-4]\d|25[0-5])){3}"
IP6 = r"(?:[0-9a-fA-F]{1,4}::?|:){1,7}(?:[0-9a-fA-F]{1,4}|(?<=:):)"
HOST = (
    rf"(?:\[?(?:(?:::f{{4,6}}:)?(?P<ip4>{IP4})|(?P<ip6>{IP6}))\]?"
    r"|(?P<dns>[\w\-.^_]*\w))"
)
FENCE = re.compile(r"^```(\w*)\n(.*?)^```", re.MULTILINE | re.DOTALL)


def blocks(text: str, language: str) -> list[str]:
    return [body for lang, body in FENCE.findall(text) if lang == language]


def settings(block: str, name: str) -> list[str]:
    pattern = re.compile(rf"^{re.escape(name)}\s*=\s*(.+?)\s*$", re.MULTILINE)
    return pattern.findall(block)


def client(failregex: str, line: str) -> str | None:
    found = re.search(failregex.replace("<HOST>", HOST), line)
    if found is None:
        return None
    return found.group("ip4") or found.group("ip6") or found.group("dns")


def json_file(line: str) -> str:
    """The line as Docker's `json-file` driver stores it."""
    entry = {"log": f"{line}\n", "stream": "stdout", "time": "2026-10-06T21:04:23.117345918Z"}
    return json.dumps(entry, separators=(",", ":"))


def problems(security: str, compose: str) -> list[str]:
    found = []
    ini = "\n".join(blocks(security, "ini"))
    regexes = settings(ini, "failregex")
    samples = [
        line
        for block in blocks(security, "text")
        for line in block.splitlines()
        if "routarr::audit:" in line and 'outcome="refused"' in line
    ]
    if not regexes:
        found.append("SECURITY.md holds no failregex")
    if len(samples) != 1:
        found.append(f"SECURITY.md holds {len(samples)} sample refusal(s), not one")
    for failregex in regexes:
        for sample in samples:
            for reading, line in [("journald", sample), ("json-file", json_file(sample))]:
                caught = client(failregex, line)
                if caught != SAMPLE_CLIENT:
                    found.append(f"{failregex!r} finds {caught!r} in the {reading} line")
            allowed = sample.replace('outcome="refused"', 'outcome="allowed"')
            for reading, line in [("journald", allowed), ("json-file", json_file(allowed))]:
                if client(failregex, line) is not None:
                    found.append(f"{failregex!r} bans on the {reading} line of an allowed event")
    for logpath in settings(ini, "logpath"):
        if "containers/*" in logpath:
            found.append(f"logpath {logpath!r} reads every container's log")
    named = re.search(r"^\s*container_name:\s*(\S+)\s*$", compose, re.MULTILINE)
    for match in settings(ini, "journalmatch"):
        if named is None or match != f"CONTAINER_NAME={named.group(1)}":
            found.append(f"journalmatch {match!r} is not the container docker-compose.yml names")
    return found


def main() -> int:
    found = problems(SECURITY.read_text(encoding="utf-8"), COMPOSE.read_text(encoding="utf-8"))
    for problem in found:
        print(f"SECURITY.md: {problem}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
