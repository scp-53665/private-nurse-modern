"""Minimal PE report: headers, sections, imports/exports, packing heuristics.

Usage: python pe_report.py <file.exe> [...]
"""
import math
import sys
from collections import Counter

import pefile


def entropy(b):
    if not b:
        return 0.0
    c = Counter(b)
    n = len(b)
    return -sum((v / n) * math.log2(v / n) for v in c.values())


def report(path):
    print(f"\n===== {path}")
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories()
    ts = pe.FILE_HEADER.TimeDateStamp
    print(f"machine=0x{pe.FILE_HEADER.Machine:04x} chars=0x{pe.FILE_HEADER.Characteristics:04x} "
          f"timestamp={ts} ep=0x{pe.OPTIONAL_HEADER.AddressOfEntryPoint:x} "
          f"imagebase=0x{pe.OPTIONAL_HEADER.ImageBase:x} subsystem={pe.OPTIONAL_HEADER.Subsystem} "
          f"linker={pe.OPTIONAL_HEADER.MajorLinkerVersion}.{pe.OPTIONAL_HEADER.MinorLinkerVersion}")
    print(f"  imageversion={pe.OPTIONAL_HEADER.MajorImageVersion}.{pe.OPTIONAL_HEADER.MinorImageVersion} "
          f"osver={pe.OPTIONAL_HEADER.MajorOperatingSystemVersion}.{pe.OPTIONAL_HEADER.MinorOperatingSystemVersion}")
    for s in pe.sections:
        name = s.Name.rstrip(b"\0").decode("latin1")
        print(f"  sect {name:<8} va=0x{s.VirtualAddress:07x} vsize=0x{s.Misc_VirtualSize:07x} "
              f"raw=0x{s.SizeOfRawData:07x} ent={entropy(s.get_data()):.2f} "
              f"chars=0x{s.Characteristics:08x}")
    if hasattr(pe, "DIRECTORY_ENTRY_IMPORT"):
        for d in pe.DIRECTORY_ENTRY_IMPORT:
            funcs = []
            for imp in d.imports:
                if imp.name:
                    funcs.append(imp.name.decode())
                else:
                    funcs.append(f"ord#{imp.ordinal}")
            print(f"  IMPORT {d.dll.decode()} ({len(funcs)}): {' '.join(funcs[:40])}"
                  + (" ..." if len(funcs) > 40 else ""))
    else:
        print("  IMPORT <none>")
    if hasattr(pe, "DIRECTORY_ENTRY_DELAY_IMPORT"):
        for d in pe.DIRECTORY_ENTRY_DELAY_IMPORT:
            print(f"  DELAY  {d.dll.decode()} ({len(d.imports)})")
    if hasattr(pe, "DIRECTORY_ENTRY_EXPORT") and pe.DIRECTORY_ENTRY_EXPORT:
        syms = [e.name.decode() if e.name else f"ord#{e.ordinal}"
                for e in pe.DIRECTORY_ENTRY_EXPORT.symbols]
        print(f"  EXPORTS ({len(syms)}): {' '.join(syms[:60])}")
    if hasattr(pe, "VS_VERSIONINFO"):
        for fo in pe.FileInfo or []:
            for entry in (fo if isinstance(fo, list) else [fo]):
                if getattr(entry, "Key", b"") == b"StringFileInfo":
                    for st in entry.StringTable:
                        for k, v in st.entries.items():
                            print(f"  VERSION {k.decode()} = {v.decode(errors='replace')}")
    pe.close()


if __name__ == "__main__":
    for p in sys.argv[1:]:
        try:
            report(p)
        except Exception as e:  # noqa: BLE001
            print(f"\n===== {p}\n  FAILED: {e}")
