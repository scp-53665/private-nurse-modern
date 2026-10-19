"""Extract the ISF opcode table (op -> mnemonic) and the handler/stub map.

The EXE embeds developer documentation entries of the form
  0xNN<TAB>MNEMONIC   : <cp932 description>\\0
covering the scenario-VM ISA (ED, LS, PM, WSH, ... EXT). See docs/02-formats.md 4.2.
Only the opcode and the mnemonic are printed: the description text is the game's
own material and is not copied into the evidence files.

The runtime VM loop is at 0x424e30: each op byte is followed by a 1-or-2-byte
prefix operand (u8, or 15-bit if the high bit is set), then dispatch through
the byte map at 0x42576c into jump table 0x42554c (136 handlers).
"""
import re
import struct
from pathlib import Path

EXE = Path(__file__).resolve().parent.parent / "original" / "NURSEUS.EXE"


def vma_off(v):
    return v - 0x401000 + 0x1000


def table():
    d = EXE.read_bytes()
    out = {}
    for m in re.finditer(rb"0x([0-9A-F]{2})\t([A-Z0-9]{1,6})[ \t]*:[ \t]*", d):
        op = int(m.group(1), 16)
        end = d.find(b"\x00", m.end())
        desc = d[m.end():end].decode("cp932", "replace").replace("\n", " ")
        out.setdefault(op, []).append((m.group(2).decode(), desc))
    return out


def handler_map():
    d = EXE.read_bytes()
    off = 0x42576C - 0x401000 + 0x1000  # .text file offset of byte map
    return d[off:off + 256]


def stub_impl_map():
    """op -> (stub_vma, impl_vma or None), read from EXE bytes.

    The 136 handler stubs are pairwise distinct code addresses (jump table at
    0x42554c); slicing the .text bytes by sorted stub starts keeps every byte
    in exactly one stub. Each stub holds at most one call (E8) to an impl, so
    the single in-range call target is that stub's implementation (ED's stub
    has none). Zero-or-one is the norm; two or more raises instead of
    misattributing.
    """
    d = EXE.read_bytes()
    tbl = [struct.unpack_from("<I", d, vma_off(0x42554C) + i * 4)[0]
           for i in range(136)]
    starts = sorted(set(tbl))
    pos = {a: i for i, a in enumerate(starts)}
    hmap = handler_map()
    out = {}
    for op in range(256):
        stub = tbl[hmap[op]]
        i = pos[stub]
        end = starts[i + 1] if i + 1 < len(starts) else stub + 64
        chunk = d[vma_off(stub):vma_off(end)]
        impl = None
        j = 0
        cands = []
        while j < len(chunk):
            if chunk[j] == 0xE8 and j + 5 <= len(chunk):
                rel, = struct.unpack_from("<i", chunk, j + 1)
                t = stub + j + 5 + rel
                if 0x424000 <= t <= 0x430000:
                    cands.append(t)
                j += 5
                continue
            j += 1
        if len(cands) == 1:
            impl = cands[0]
        elif len(cands) > 1:
            raise AssertionError(f"op {op:#04x}: {len(cands)} call candidates")
        out[op] = (stub, impl)
    return out


def main():
    tab = table()
    hmap = handler_map()
    smap = stub_impl_map()
    n_rows = sum(len(v) for v in tab.values())
    print(f"ops={len(tab)} rows={n_rows}")
    for op in sorted(tab):
        for mn, _desc in tab[op]:
            stub, impl = smap[op]
            print(f"0x{op:02X} {mn:<6} h={hmap[op]:3d} stub={stub:#x} "
                  f"impl={impl if impl is None else hex(impl)}")
    gaps = [op for op in range(256) if op not in tab]
    print(f"unassigned={len(gaps)}")
    print("--- undocumented ops with their own impl (not the reset stub)")
    for op in sorted(gaps):
        stub, impl = smap[op]
        if impl not in (None, 0x424E20):
            print(f"0x{op:02X} h={hmap[op]:3d} stub={stub:#x} impl={hex(impl)}")


if __name__ == "__main__":
    assert len(table()) > 100, "opcode table not found"
    assert len(handler_map()) == 256
    main()
