"""Build the release zip (docs/04-l2-presenter.md §1, §8) and check what goes into it.

    python tools/release.py [OUT.zip]  -> target/dist/pn-modern-<version>.zip (or OUT.zip)

Builds the hook DLL (i686: the game is 32-bit) and PN-Patch.exe (x86_64: a 32-bit EXE
without a manifest is UAC-virtualized and would install into VirtualStore), runs the
tests, then asserts: the DLL is i386 with exactly the stock shim's three undecorated
exports (the host IAT binds by name, review Y1), PN-Patch is x64, and the DLL carries the
`pn_modern` marker PN-Patch uses to recognise its DLL. Exits non-zero on any failure.
"""
import os
import re
import subprocess
import sys
import zipfile
from pathlib import Path

import pefile

ROOT = Path(__file__).resolve().parent.parent
DLL = ROOT / "target/i686-pc-windows-msvc/release/pn_modern.dll"
EXE = ROOT / "target/release/PN-Patch.exe"
EXPORTS = [b"mciSendCommandA", b"mciSendStringA", b"mixerSetControlDetails"]
DOCS = ["README.md", "LICENSE-MIT"]


def cargo(*args):
    print("+ cargo", " ".join(args), flush=True)
    subprocess.run(["cargo", *args], cwd=ROOT, check=True)


def check():
    dll = pefile.PE(str(DLL))
    assert dll.FILE_HEADER.Machine == 0x14C, "DLL must be i386"
    names = sorted(e.name for e in dll.DIRECTORY_ENTRY_EXPORT.symbols)
    assert names == EXPORTS, f"DLL exports {names}"
    exe = pefile.PE(str(EXE))
    assert exe.FILE_HEADER.Machine == 0x8664, "PN-Patch must be x64"
    # Neither binary may need a VC++ redistributable (review s14 S14-3).
    for name, pe in (("DLL", dll), ("PN-Patch", exe)):
        needs = [d.dll.decode() for d in pe.DIRECTORY_ENTRY_IMPORT
                 if re.match(r"(?i)vcruntime|msvcp|api-ms-win-crt|ucrtbase", d.dll.decode())]
        assert not needs, f"{name} imports the dynamic CRT: {needs}"
    assert b"pn_modern" in DLL.read_bytes(), "DLL lacks the marker PN-Patch looks for"


def main():
    cargo("build", "--release", "--target", "i686-pc-windows-msvc", "-p", "pn_presenter")
    cargo("build", "--release", "-p", "pn_config")
    # --release: the gate must test the profile that ships (s14b m-2).
    cargo("test", "--release", "--target", "i686-pc-windows-msvc", "-p", "pn_presenter", "-p", "pn_shared")
    cargo("test", "--release", "-p", "pn_config")
    check()
    toml = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    version = re.search(r'^version = "([^"]+)"', toml, re.M).group(1)
    out = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / f"target/dist/pn-modern-{version}.zip"
    out.parent.mkdir(parents=True, exist_ok=True)
    # Written beside the target and moved into place: a failure half way through must not
    # leave a truncated file under the release name (s14b m-3).
    tmp = out.with_name(out.name + ".tmp")
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(DLL, "pn_modern.dll")
        z.write(EXE, "PN-Patch.exe")
        for d in DOCS:
            z.write(ROOT / d, d)
    os.replace(tmp, out)
    with zipfile.ZipFile(out) as z:
        listed = sorted(z.namelist())
    assert listed == sorted(["pn_modern.dll", "PN-Patch.exe", *DOCS]), listed
    print(f"release: {out} ({out.stat().st_size} bytes) {listed}")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, subprocess.CalledProcessError, OSError) as e:
        sys.exit(f"release: FAILED: {e}")
