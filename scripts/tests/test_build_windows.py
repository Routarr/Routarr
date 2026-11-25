"""`scripts/build-windows.py`, the Windows zip and installer, and `packaging/windows/`."""

import hashlib
import re
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path

from _scripts import ROOT, load

build_windows = load("build-windows")


def pe(machine: int) -> bytes:
    """The head of a PE image built for `machine`."""
    head = bytearray(0x100)
    head[:2] = b"MZ"
    struct.pack_into("<I", head, 0x3C, 0x80)
    head[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<H", head, 0x84, machine)
    return bytes(head)


def service_sid(name: str) -> str:
    """The SID Windows gives the virtual account `NT SERVICE\\name`."""
    digest = hashlib.sha1(name.upper().encode("utf-16-le")).digest()
    return "S-1-5-80-" + "-".join(str(part) for part in struct.unpack("<5I", digest))


class BuildWindows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.exe = self.tmp / "routarr.exe"
        self.exe.write_bytes(pe(build_windows.AMD64))
        self.frontend = self.tmp / "dist"
        (self.frontend / "assets").mkdir(parents=True)
        (self.frontend / "index.html").write_text("<!doctype html>")
        (self.frontend / "assets" / "app.js").write_text("export {}")
        (self.frontend / "third-party-notices.txt").write_text("packages\n")
        self.crates = self.tmp / "crates.txt"
        self.crates.write_text("crates\n")

    def tearDown(self):
        import shutil

        shutil.rmtree(self.tmp)

    def zipped(self, epoch=1_700_000_000):
        stage = self.tmp / "stage"
        out = self.tmp / f"out-{len(list(self.tmp.glob('out-*')))}"
        stage.mkdir(exist_ok=True)
        out.mkdir()
        if not (stage / "routarr.exe").exists():
            build_windows.stage(stage, self.exe, self.frontend, self.crates)
        return build_windows.write_zip(stage, out, "routarr-0.3.2-windows-x64", epoch)

    def test_the_zip_holds_the_binary_its_interface_and_their_notices(self):
        archive = self.zipped()
        with zipfile.ZipFile(archive) as zipped:
            names = sorted(zipped.namelist())
            notices = zipped.read(
                "routarr-0.3.2-windows-x64/third-party-notices.txt"
            ).decode()
            served = zipped.read(
                "routarr-0.3.2-windows-x64/frontend/dist/third-party-notices.txt"
            ).decode()
        folder = "routarr-0.3.2-windows-x64/"
        expected = [
            "LICENSE",
            "frontend/dist/assets/app.js",
            "frontend/dist/index.html",
            "frontend/dist/third-party-notices.txt",
            "routarr.exe",
            "third-party-notices.txt",
        ]
        self.assertEqual(names, [folder + name for name in expected])
        self.assertEqual(notices, "crates\npackages\n")
        self.assertEqual(
            served, notices, "the interface serves other notices than it ships"
        )

    def test_one_commit_zips_into_the_same_bytes_twice(self):
        first = self.zipped().read_bytes()
        again = self.zipped().read_bytes()
        self.assertEqual(first, again)
        self.assertNotEqual(
            first, self.zipped(epoch=1_800_000_000).read_bytes(), "the control"
        )

    def test_a_binary_for_another_machine_is_refused(self):
        for head in (pe(0x014C), pe(0xAA64), b"#!/bin/sh\n"):
            with self.subTest(head[:8]):
                self.exe.write_bytes(head)
                stage = self.tmp / f"stage-{head[:2].hex()}-{len(head)}"
                stage.mkdir(exist_ok=True)
                with self.assertRaises(SystemExit):
                    build_windows.stage(stage, self.exe, self.frontend, self.crates)

    def test_a_release_candidate_installs_as_its_release(self):
        self.assertEqual(build_windows.msi_version("0.3.2"), "0.3.2")
        self.assertEqual(build_windows.msi_version("0.3.2-rc.1"), "0.3.2")
        with self.assertRaises(SystemExit):
            build_windows.msi_version("next")


class Installer(unittest.TestCase):
    """What the MSI sets and the binary reads, written in two languages."""

    def setUp(self):
        self.wxs = (ROOT / "packaging" / "windows" / "routarr.wxs").read_text()
        service = (ROOT / "backend" / "src" / "service.rs").read_text()
        self.name = re.search(r'pub const NAME: &str = "([^"]+)";', service).group(1)

    def test_the_service_runs_as_the_account_its_name_gives(self):
        installed = re.search(r"<ServiceInstall [^>]*>", self.wxs, re.DOTALL).group(0)
        self.assertIn(f'Name="{self.name}"', installed)
        self.assertIn(f'Account="NT SERVICE\\{self.name}"', installed)
        self.assertIn('Arguments="service"', installed)

    def test_the_data_folder_lets_in_the_service_by_the_sid_of_its_name(self):
        sddl = re.search(r'Sddl="([^"]+)"', self.wxs).group(1)
        self.assertTrue(
            sddl.startswith("D:P"), "the folder inherits its parent's permissions"
        )
        self.assertEqual(
            sorted(re.findall(r"\(A;OICI;FA;;;([^)]+)\)", sddl)),
            sorted(["SY", "BA", service_sid(self.name)]),
        )

    def test_the_sid_is_derived_as_windows_derives_it(self):
        self.assertEqual(
            service_sid("TrustedInstaller"),
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
        )


if __name__ == "__main__":
    unittest.main()
