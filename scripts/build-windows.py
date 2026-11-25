#!/usr/bin/env python3
"""Build Routarr's Windows installer and portable zip from a release build.

    python3 scripts/build-windows.py wix DIR
    python3 scripts/build-windows.py package --version X.Y.Z --exe routarr.exe \\
        --frontend frontend/dist --crates-notices crates.txt --out DIR [--wix DIR]

`wix` fetches the WiX Toolset and its Util extension into DIR, each checked against the SHA-512
NuGet's catalog publishes for it. `package` stages what Windows gets, writes
`routarr-X.Y.Z-windows-x64.zip` into OUT and, given the WiX folder, the MSI beside it
(`packaging/windows/routarr.wxs`). Building the MSI accepts the WiX maintenance fee's EULA,
which exempts a user under 10,000 USD of annual revenue.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WXS = ROOT / "packaging" / "windows" / "routarr.wxs"
SETTINGS = ROOT / "packaging" / "windows" / "routarr.env"

WIX_VERSION = "7.0.1"
# Folder in DIR, NuGet id, SHA-512 of the package.
WIX_PACKAGES = (
    (
        "wix",
        "wix",
        (
            "9c72a17d435e0c642f2ba6d78424a49526fcd743cc2d04bd1e053d1ecae2dd12"
            "d63faa6201c79577490e37756677f5a8d7df7f5498e64bf996018ebe17b37d70"
        ),
    ),
    (
        "util",
        "wixtoolset.util.wixext",
        (
            "8886a43a14d63ac2a48905162d1c95121fd1a4125a2168dd98e2b200c7614f4d"
            "6fad96845a88d4e1eb64db9f571f93390b5f67bd3412984c1497d4d95898103d"
        ),
    ),
    (
        "firewall",
        "wixtoolset.firewall.wixext",
        (
            "875dfabfab488e73a2748284825f3889a137cc52fe734a84ecee93686907164c"
            "1b1a02e78f28c81a4f6f78eeefa63c5170fa0d529e8515989aee6f869becf0e4"
        ),
    ),
)
WIX_TOOL = Path("wix/tools/net8.0/any/wix.dll")
WIX_EXTENSIONS = (
    Path("util/wixext7/WixToolset.Util.wixext.dll"),
    Path("firewall/wixext7/WixToolset.Firewall.wixext.dll"),
)
FEED = "https://api.nuget.org/v3-flatcontainer/{id}/{version}/{id}.{version}.nupkg"

# The PE header's machine field of an x86-64 image.
AMD64 = 0x8664


def fetch_wix(dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    for folder, package, sha512 in WIX_PACKAGES:
        url = FEED.format(id=package, version=WIX_VERSION)
        with urllib.request.urlopen(url, timeout=120) as response:
            body = response.read()
        found = hashlib.sha512(body).hexdigest()
        if found != sha512:
            raise SystemExit(
                f"{package} {WIX_VERSION} has SHA-512 {found}, not {sha512}"
            )
        archive = dest / f"{package}.{WIX_VERSION}.nupkg"
        archive.write_bytes(body)
        with zipfile.ZipFile(archive) as unpacked:
            unpacked.extractall(dest / folder)
    print(f"WiX {WIX_VERSION} in {dest}")


def pe_machine(exe: Path) -> int | None:
    """The machine field of a PE image, or None for a file that is none."""
    head = exe.read_bytes()[:4096]
    if head[:2] != b"MZ" or len(head) < 0x40:
        return None
    (offset,) = struct.unpack_from("<I", head, 0x3C)
    if head[offset : offset + 4] != b"PE\0\0":
        return None
    (machine,) = struct.unpack_from("<H", head, offset + 4)
    return machine


def msi_version(version: str) -> str:
    """The X.Y.Z an MSI holds: Windows reads no pre-release, so an rc installs as its release."""
    found = re.match(r"(\d+)\.(\d+)\.(\d+)", version)
    if not found:
        raise SystemExit(f"{version} is no X.Y.Z version")
    return found.group(0)


def stage(stage_dir: Path, exe: Path, frontend: Path, crates_notices: Path) -> None:
    """Lay out what the installer and the zip ship. The interface's notices list the packages it
    bundles: the crates' come first, in one file served beside the interface and kept beside the
    binary."""
    machine = pe_machine(exe)
    if machine != AMD64:
        shown = "no PE image" if machine is None else f"PE machine {machine:#06x}"
        raise SystemExit(f"{exe} is no x86-64 Windows binary ({shown})")
    shutil.copy2(exe, stage_dir / "routarr.exe")
    dist = stage_dir / "frontend" / "dist"
    shutil.copytree(frontend, dist)
    packages = dist / "third-party-notices.txt"
    bundled = packages.read_text(encoding="utf-8") if packages.exists() else ""
    notices = crates_notices.read_text(encoding="utf-8") + bundled
    packages.write_text(notices, encoding="utf-8", newline="\n")
    (stage_dir / "third-party-notices.txt").write_text(
        notices, encoding="utf-8", newline="\n"
    )
    shutil.copy2(ROOT / "LICENSE", stage_dir / "LICENSE")
    shutil.copy2(SETTINGS, stage_dir / "routarr.env")


def write_zip(stage_dir: Path, out: Path, name: str, epoch: int) -> Path:
    """The portable zip: everything but the installer's settings, under one folder, each entry
    dated `epoch` so one commit zips into the same bytes twice."""
    archive = out / f"{name}.zip"
    stamp = time.gmtime(max(epoch, 315532800))[:6]
    files = sorted(
        path
        for path in stage_dir.rglob("*")
        if path.is_file() and path.name != "routarr.env"
    )
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
        for path in files:
            entry = zipfile.ZipInfo(
                f"{name}/{path.relative_to(stage_dir).as_posix()}", stamp
            )
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o644 << 16
            zipped.writestr(entry, path.read_bytes())
    return archive


def write_msi(stage_dir: Path, out: Path, name: str, version: str, wix: Path) -> Path:
    msi = out / f"{name}.msi"
    command = [
        "dotnet",
        str(wix / WIX_TOOL),
        "build",
        str(WXS),
        "-arch",
        "x64",
        "-acceptEula",
        "wix7",
        *(
            part
            for extension in WIX_EXTENSIONS
            for part in ("-ext", str(wix / extension))
        ),
        "-d",
        f"Version={msi_version(version)}",
        "-d",
        f"Stage={stage_dir}",
        "-o",
        str(msi),
    ]
    subprocess.run(command, check=True)
    return msi


def commit_epoch() -> int:
    """The commit's time, or now outside a checkout."""
    if "SOURCE_DATE_EPOCH" in os.environ:
        return int(os.environ["SOURCE_DATE_EPOCH"])
    shown = subprocess.run(
        ["git", "-C", str(ROOT), "log", "-1", "--format=%ct"],
        capture_output=True,
        text=True,
        check=False,
    )
    return int(shown.stdout.strip()) if shown.returncode == 0 else int(time.time())


def package(args: argparse.Namespace) -> None:
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    name = f"routarr-{args.version}-windows-x64"
    with tempfile.TemporaryDirectory() as staged:
        stage_dir = Path(staged)
        stage(stage_dir, args.exe, args.frontend, args.crates_notices)
        print(write_zip(stage_dir, out, name, commit_epoch()))
        if args.wix:
            print(write_msi(stage_dir, out, name, args.version, args.wix.resolve()))


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    wix = commands.add_parser("wix", help="fetch the WiX Toolset")
    wix.add_argument("dir", type=Path)
    built = commands.add_parser("package", help="build the zip and the MSI")
    built.add_argument("--version", required=True)
    built.add_argument("--exe", type=Path, required=True)
    built.add_argument("--frontend", type=Path, required=True)
    built.add_argument("--crates-notices", type=Path, required=True)
    built.add_argument("--out", type=Path, required=True)
    built.add_argument("--wix", type=Path)
    args = parser.parse_args(argv)
    if args.command == "wix":
        fetch_wix(args.dir)
    else:
        package(args)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
