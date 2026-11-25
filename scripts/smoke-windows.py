#!/usr/bin/env python3
"""Install Routarr's MSI and its portable zip, check what each does, and remove them.

    python scripts/smoke-windows.py MSI ZIP [NEXT_MSI]

Windows only, as an administrator, on a machine of no other use such as a CI runner. The MSI must
leave a running service under its virtual account answering on port 9876, the first-run command
of `ApiKeyGate.svelte` printing its key, its data folder closed to every other user, the stop
and recovery settings it declares, a clean stop, and its data and settings kept once removed.
NEXT_MSI, a later version, must upgrade it in place. The zip must run where it is unpacked,
close a data folder it makes, and stop on Ctrl-Break.
"""

from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GATE = ROOT / "frontend" / "src" / "components" / "ApiKeyGate.svelte"
SERVICE = "Routarr"
DATA = Path(os.environ.get("ProgramData", r"C:\ProgramData")) / "Routarr"
PROGRAM = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Routarr"
# Groups a new folder under a drive's root lets in, which a closed folder no longer names.
OTHERS = ("BUILTIN\\Users", "Authenticated Users", "Everyone")


def run(*command: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    done = subprocess.run(command, capture_output=True, text=True, check=False)
    if check and done.returncode != 0:
        raise SystemExit(
            f"{' '.join(command)} ended with {done.returncode}:\n{done.stdout}{done.stderr}"
        )
    return done


def check(holds: bool, what: str) -> None:
    if not holds:
        raise SystemExit(f"FAILED: {what}")
    print(f"ok: {what}")


def until(what: str, reached, seconds: float = 90) -> None:
    deadline = time.monotonic() + seconds
    while not reached():
        if time.monotonic() > deadline:
            raise SystemExit(f"FAILED: never {what} within {seconds:.0f} s")
        time.sleep(1)
    print(f"ok: {what}")


def service_state() -> str | None:
    """RUNNING, STOPPED and the like, or None once no such service exists."""
    shown = run("sc.exe", "query", SERVICE, check=False)
    found = re.search(r"STATE\s+:\s+\d+\s+(\w+)", shown.stdout)
    return found.group(1) if found else None


def answers(port: int) -> bool:
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/api/v1/ping", timeout=3
        ) as reply:
            return reply.status == 200
    except OSError:
        return False


def get_json(port: int, path: str) -> dict:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=5) as reply:
        return json.load(reply)


def acl(path: Path) -> str:
    return run("icacls", str(path)).stdout


def key_command() -> str:
    """The first-run screen's command for the Windows installer, as the interface shows it."""
    found = re.search(
        r"const WINDOWS_KEY_COMMAND = '(.*)';", GATE.read_text(encoding="utf-8")
    )
    if not found:
        raise SystemExit(f"{GATE} names no WINDOWS_KEY_COMMAND")
    return found.group(1).replace("\\\\", "\\")


def install(msi: Path, log: Path) -> None:
    done = run(
        "msiexec", "/i", str(msi), "/qn", "/norestart", "/l*v", str(log), check=False
    )
    if done.returncode != 0:
        tail = (
            log.read_text(encoding="utf-16", errors="replace")[-4000:]
            if log.exists()
            else ""
        )
        raise SystemExit(f"msiexec /i ended with {done.returncode}\n{tail}")


def refused_start() -> None:
    """A variable the service refuses ends its start with the status 78, which the recovery
    leaves stopped, and the log it never opened still says why."""
    settings = DATA / "routarr.env"
    kept = settings.read_text(encoding="utf-8")
    # The first line naming a variable is the one read, so the line itself changes.
    settings.write_text(
        kept.replace("ROUTARR_PORT=9876", "ROUTARR_PORT=not-a-port"), encoding="utf-8"
    )
    try:
        run("sc.exe", "start", SERVICE, check=False)
        until(
            "a refused configuration stops it", lambda: service_state() == "STOPPED", 60
        )
        stopped = run("sc.exe", "query", SERVICE).stdout
        check(
            re.search(r"SERVICE_EXIT_CODE\s+:\s+78\b", stopped) is not None,
            "it stops with the status 78",
        )
        # Past the recovery's 30 s delay.
        time.sleep(45)
        check(
            service_state() == "STOPPED",
            "the recovery leaves a refused configuration stopped",
        )
        log = (DATA / "routarr.log").read_text(encoding="utf-8", errors="replace")
        check(
            "Routarr did not start" in log and "ROUTARR_PORT" in log, "its log says why"
        )
    finally:
        settings.write_text(kept, encoding="utf-8")


def installed_versions() -> list[str]:
    """The version of each product named Routarr in the machine's list of installed programs."""
    listed = run(
        "powershell",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        r"Get-ItemProperty HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\* "
        "| Where-Object DisplayName -eq Routarr | ForEach-Object DisplayVersion",
    ).stdout
    return listed.split()


def upgraded(msi: Path, logs: Path) -> None:
    """The next release over this one: one Routarr installed, its service back as its own
    account, its key and a line of settings written by hand kept."""
    settings = DATA / "routarr.env"
    by_hand = "# Written by hand, kept across an upgrade.\n"
    settings.write_text(
        settings.read_text(encoding="utf-8") + by_hand, encoding="utf-8"
    )
    key = (DATA / "routarr.api_key").read_text(encoding="utf-8").strip()
    install(msi, logs / "upgrade.log")
    until("the upgraded service runs", lambda: service_state() == "RUNNING")
    until("it answers again", lambda: answers(9876))
    version = re.match(r"routarr-(.+)-windows-x64\.msi$", msi.name).group(1)
    check(installed_versions() == [version], f"one Routarr is installed, {version}")
    check(
        by_hand in settings.read_text(encoding="utf-8"),
        "the settings written by hand stay",
    )
    check(
        (DATA / "routarr.api_key").read_text(encoding="utf-8").strip() == key,
        "the key stays",
    )
    config = run("sc.exe", "qc", SERVICE).stdout
    check(f"NT SERVICE\\{SERVICE}" in config, "it still runs as its virtual account")


def the_msi(msi: Path, logs: Path, upgrade: Path | None) -> None:
    install(msi, logs / "install.log")
    until("the service runs", lambda: service_state() == "RUNNING")
    until("Routarr answers on port 9876", lambda: answers(9876))
    check(
        get_json(9876, "/api/v1/auth/mode")["install"] == "windows",
        "it says the installer",
    )

    config = run("sc.exe", "qc", SERVICE).stdout
    check(f"NT SERVICE\\{SERVICE}" in config, "it runs as its virtual account")
    check(
        re.search(r'routarr\.exe"?\s+service', config) is not None,
        "Windows starts it as a service",
    )
    failure = run("sc.exe", "qfailure", SERVICE).stdout
    check("RESTART" in failure, "a crash restarts it")
    preshutdown = run(
        "reg",
        "query",
        rf"HKLM\SYSTEM\CurrentControlSet\Services\{SERVICE}",
        "/v",
        "PreshutdownTimeout",
        check=False,
    ).stdout
    check("0xea60" in preshutdown, "a shutdown waits a minute for it")

    key = run(
        "powershell", "-NoProfile", "-NonInteractive", "-Command", key_command()
    ).stdout.strip()
    check(
        re.fullmatch(r"[0-9a-f]{64}", key) is not None,
        "the first-run command prints the key",
    )
    held = acl(DATA)
    check(
        not any(group in held for group in OTHERS),
        "the data folder is closed to other users",
    )
    check(
        f"NT SERVICE\\{SERVICE}" in held and "BUILTIN\\Administrators" in held,
        "the service and the administrators may open it",
    )
    check(
        not any("(I)" in line for line in held.splitlines()[:4]), "it inherits nothing"
    )

    run("sc.exe", "stop", SERVICE)
    until("the service stops", lambda: service_state() == "STOPPED", 60)
    stopped = run("sc.exe", "query", SERVICE).stdout
    check(
        re.search(r"WIN32_EXIT_CODE\s+:\s+0\b", stopped) is not None,
        "it stops with no error",
    )
    log = (DATA / "routarr.log").read_text(encoding="utf-8", errors="replace")
    check("Routarr stopped cleanly" in log, "it stops cleanly, as its log says")
    refused_start()

    run("sc.exe", "start", SERVICE)
    until("it starts again", lambda: answers(9876))
    if upgrade:
        upgraded(upgrade, logs)
        msi = upgrade
    done = run(
        "msiexec",
        "/x",
        str(msi),
        "/qn",
        "/norestart",
        "/l*v",
        str(logs / "remove.log"),
        check=False,
    )
    check(done.returncode == 0, "msiexec /x removes it")
    check(service_state() is None, "the service is gone")
    check(not (PROGRAM / "routarr.exe").exists(), "the program is gone")
    check(
        (DATA / "routarr.db").exists() and (DATA / "routarr.env").exists(),
        "the library and the settings stay",
    )


def the_zip(archive: Path, work: Path) -> None:
    with zipfile.ZipFile(archive) as zipped:
        zipped.extractall(work)
    exe = next(work.glob("routarr-*-windows-x64/routarr.exe"))
    # Under the drive's root, where a new folder lets every user read it until it is closed.
    control = Path(r"C:\routarr-smoke-control")
    control.mkdir(exist_ok=True)
    check(
        any(group in acl(control) for group in OTHERS),
        "the control: a new folder there is open",
    )
    data = Path(r"C:\routarr-smoke-data")
    env = dict(
        os.environ,
        ROUTARR_DB_PATH=str(data / "routarr.db"),
        ROUTARR_PORT="19876",
        ROUTARR_HOST="127.0.0.1",
    )
    output = work / "routarr.out"
    with output.open("w") as sink:
        server = subprocess.Popen(
            [str(exe)],
            cwd=exe.parent,
            env=env,
            stdout=sink,
            stderr=subprocess.STDOUT,
            creationflags=subprocess.CREATE_NEW_PROCESS_GROUP,
        )
        try:
            until("the zip's binary answers", lambda: answers(19876))
            check(
                get_json(19876, "/api/v1/auth/mode")["install"] is None,
                "it names no installer",
            )
            check(
                not any(group in acl(data) for group in OTHERS),
                "the data folder it made is closed",
            )
            server.send_signal(signal.CTRL_BREAK_EVENT)
            code = server.wait(timeout=60)
        finally:
            if server.poll() is None:
                server.kill()
    said = output.read_text(errors="replace")
    check(
        code == 0 and "Routarr stopped cleanly" in said,
        f"Ctrl-Break stops it cleanly\n{said[-2000:]}",
    )


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3) or sys.platform != "win32":
        print(__doc__, file=sys.stderr)
        return 2
    msi, archive, *upgrade = (Path(arg).resolve() for arg in argv)
    with tempfile.TemporaryDirectory() as scratch:
        work = Path(scratch)
        the_msi(msi, work, upgrade[0] if upgrade else None)
        the_zip(archive, work)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
