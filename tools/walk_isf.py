"""ISF bytecode walker (U2). Tiles the bytes between text runs as ops.

Model, all from NURSEUS.EXE disassembly: VM loop at 0x424e30 reads op byte +
prefix (u8, or 15-bit if hi-bit set), then the handler stub's impl reads its
own operands. Text runs (pairs + lone terminator) are data, not ops.

EXTRA holds only operand sizes verified against the impl bodies (ground truth
re-checked via objdump --start-address per impl, not the linear exe.asm dump
which desyncs in places). Ops whose stub calls nothing (ED, HXP) or just the
reset take op+prefix only (derived from EXE bytes via probe_isf_opcodes).
ONJS/ONJP take VAL4 + u8 count + 2*count table bytes (static length,
state-dependent target). A 1-2 byte tail before a text marker that completes
no known op is a message tag ("TAG"), not code: nonzero tags occur only
before quoted lines and cluster per scene. --scan reports tiling per file;
--scene NAME disassembles one scene end to end (U2 acceptance).
"""
import collections
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
from decode_isf_text import text_runs, char
from probe_isf_opcodes import stub_impl_map, table as op_table
from unpack import read_sm2mpx

SRC = ROOT / "original" / "ISF"

# op -> extra operand bytes beyond the loop prefix. int = fixed bytes,
# "str" = NUL-terminated string <=13+1, tuple = sequence of int/"s13"/"s28"
# steps ("s28" = NUL-terminated <=28+1 via the second string reader 0x4249f0).
# Only sizes verified against the impl bodies; see module docstring.
EXTRA = {
    0x04: 2, 0x05: 2,                   # JP, JS: one u16 (0x424990)
    0x2B: 1, 0x2C: 1,                   # PM, PMP: one u8 (0x424970)
    0x01: "str", 0x02: "str",           # LS, LSBS: C-string (0x4249c0)
    0x31: 3,                            # SK: u16 + u8; impl 0x4272c0
    0x32: 5,                            # SKS: u16 + u16 + u8; impl 0x427300
    0x10: 18,                           # CW: u8 + 4xVAL4 + u8; impl 0x426420
    0x85: 6,                            # IG: u16 + u16 + u8 + u8; impl 0x42a5c0
    0x5D: 9,                            # GASTOP: u8 + VAL4 + VAL4; impl 0x4294c0
    0x4A: 13,                           # EXC: 3xVAL4 + u8; impl 0x4288b0
    0x62: 57,                           # GSCRL: u8 + 14xVAL4; impl 0x429640
    0xA3: 4,                            # KIDSET: one raw u32 (0x424980); impl 0x42a9f0
    0x59: 4,                            # GPB: VAL4; impl 0x429370
    0x55: 7,                            # GO: VAL4 + 3xu8; impl 0x428ea0
    0x4C: 3,                            # SSP: u16 + u8 (+0x42c430 value helper); impl 0x428c50
    0xE5: 1,                            # undocumented: u8; impl 0x42b2a0
    0x46: 6,                            # HT: 3xu16; impl 0x4282a0
    0x24: 2,                            # CN: u8 + u8; impl 0x426e80
    0x2A: 1,                            # WC: u8; impl 0x4270f0
    0x42: 2,                            # HINC: u16 (+0x42c3f0 value helper); impl 0x428090
    0x3A: 4,                            # STC: u8 + u8 + u16; impl 0x427d60
    0x52: 0,                            # GF: no operands (impl 0x428d40 only resets)
    0x72: 4,                            # MF: VAL4; impl 0x429d20
    0x80: (1, "s13"),                   # IM: u8 + STR13; impl 0x42a2e0
    0x12: (2, "s13"),                   # CIR: u8 + u8 + STR13; impl 0x426690
    0x22: (1, "s13"),                   # WL: u8 + STR13; impl 0x426d80
    0xE0: (1, "s28"),                   # TAGSET: u8 + STR28; impl 0x42ac10
    0xE4: (4, "s28", "s28", "s28", 1, 4, 4, 4, 1, 2, 2),  # undoc: 4xu8 + 3xSTR28 + u8 + 3xVAL4 + u8 + 2xu16; impl 0x42b090
    0x73: 0,                            # MS: no operands (impl 0x429d90 only resets)
    0x7A: 0,                            # PCMEND: no operands (impl 0x42a2b0 only resets)
    0x03: 0,                            # SRET: no operands (sub 0x424b00 touches no stream)
    0x68: 0,                            # undocumented: no operands (impl 0x429920 only resets)
    0x34: 6,                            # FT: 3xu16; impl 0x4273c0
    0xA1: 6,                            # undocumented: 6xu8; impl 0x42a840
    0xFC: 2,                            # undocumented: u16 (+0x4246d0/0x42c390 value helpers); impl 0x42bda0
    0x5A: 1,                            # GPJ: u8; impl 0x4293a0
    0x43: 2,                            # HDEC: u16 (+0x42c410 value helper); impl 0x4280c0
    0x82: 16,                           # IMS: 4xVAL4; impl 0x42a360
    0x92: 4,                            # DAS: VAL4; impl 0x42a7d0
    0x69: 4,                            # undocumented: VAL4; impl 0x429950
    0x3B: 4,                            # HN: u16 + u16 (jump via 0x425870 reads no stream); impl 0x427360
    0x2D: 4,                            # WSH: VAL4; impl 0x4271f0
    0x2E: 4,                            # WSS: VAL4; impl 0x427230
    0x26: (1, 4),                        # PF: u8 + VAL4; impl 0x426f40
    0x27: (1, 4),                        # PB: u8 + VAL4; impl 0x426fd0
    0x28: (1, 1),                        # PJ: u8 + u8; impl 0x427020
    0x29: 1,                            # WO: u8; impl 0x4270b0
    0x09: "str",                         # undocumented: STR13 (sub 0x425bb0 touches no stream); impl 0x426380
    0x5C: (1, 4, 4),                     # undocumented: u8 + 2xVAL4; impl 0x429450
    0xE2: (1, 1, 1, 2, "s28"),           # RBSET: 3xu8 + u16 + STR28; impl 0x42adc0
    0x23: (1, 4, 4, 1),                  # WW: u8 + 2xVAL4 + u8; impl 0x426de0
    0xA2: (1, 1, 1, 1, 1, 1, 2, 1, 2, 2),  # undocumented: 6xu8 + u16 + u8 + 2xu16; impl 0x42a8f0
    0x18: (1, 2, 1),                     # CC: u8 + u16 + u8 (+0x4259d0/0x4259e0 field helpers); impl 0x426af0
    0x14: (1, 1, 4, 4, 4),               # CIP: u8 + u8 + 3xVAL4; impl 0x426890
    0x91: (4, 1),                         # DAP: VAL4 + u8; impl 0x42a6e0
    0xF2: 0,                              # AWAIT: no operands (shares EXT impl 0x42bde0)
    0xFF: 0,                              # EXT: no operands; impl 0x42bde0
    0x25: (1, 1),                         # CNS: u8 + u8; impl 0x426ee0
    0x20: (1, 4, 4, 4, 4, 1),             # WS: u8 + 4xVAL4 + u8; impl 0x426bf0
    0xFA: 4,                              # undocumented: VAL4 (+0x424690 value helper); impl 0x42bd60
    0x33: 4,                              # HF: u16 + u16 (shares HN impl 0x427360; jump via 0x425870 reads no stream)
    0x4B: (2, 1),                         # SCP: u16 + u8 (+value helpers); impl 0x428b10
    0x63: (2, 1, 1, 4),                   # GV: u16 + u8 + u8 + VAL4; impl 0x429770
    0xE8: 3,                              # undocumented: 3xu8; impl 0x42b380
    0xF0: (1, "s13"),                     # CNF: u8 + STR13; impl 0x42b410
    0x13: 19,                             # CPS: 19xu8; impl 0x426720 (ground truth via --start-address; exe.asm desyncs here)
    0x83: (4, 4),                         # IXY: 2xVAL4; impl 0x42a450
    0x53: (4, 1, 1, 1),                   # GC: VAL4 + 3xu8; impl 0x428d70
    0x30: 2,                              # FLN: u16; impl 0x427270
    0x11: (1, 1, "s13"),                  # CP: u8 + u8 + STR13; impl 0x4265f0
    0xF4: 1,                              # PPF: u8; impl 0x42bb70
    0x48: (2, 2),                         # EXA: u16 + u16; impl 0x4283f0
    0x49: (4, 4, 4, 1),                   # EXS: 3xVAL4 + u8 (+0x424780 value helper); impl 0x428850
    0xA4: 0,                              # KIDEND: no operands (impl 0x42aa60 only resets)
    0x40: 2,                              # HLN: u16; impl 0x428000 (ground truth via --start-address; exe.asm desyncs here)
    0x38: 4,                              # ES: 2xu16; impl 0x4279d0
    0xA5: 4,                              # KIDFN: u32; impl 0x42aaa0
    0xF8: 2,                              # undocumented: 2xu8; impl 0x42bbd0
    0x5B: 12,                             # PR: 3xVAL4; impl 0x4293f0
    0x19: 2,                              # CCLR: 2xu8; impl 0x426b90
    0x15: (1, 1, 4, 4, 4, 4),             # CSET: u8 + u8 + 4xVAL4; impl 0x426910
    0x70: ("s13", 1),                     # ML: STR13 + u8; impl 0x429980
    0x74: ("s13", 4),                     # SER: STR13 + VAL4; impl 0x429de0
    0x35: 1,                              # SP: u8; impl 0x427410
    0x64: (4, 1),                         # GAL: VAL4 + u8; impl 0x4297d0
    0x37: (1, 1),                         # STS: 2xu8; impl 0x427500
    0x39: (2, 2),                         # EC: 2xu16; impl 0x427be0
    0x54: (4, 4, 1, 1, 1),                # GI: 2xVAL4 + 3xu8; impl 0x428df0
    0x90: (4, 4),                         # DAE: 2xVAL4; impl 0x42a6a0
    0x41: (2, 4),                         # HS: u16 + VAL4; impl 0x428050
    0xE3: (1, 1, 1, 2, "s28"),            # CBSET: 3xu8 + u16 + STR28; impl 0x42af50
    0x67: (4, 2, 2),                      # GAPOS: VAL4 + 2xu16; impl 0x4298d0
    0x45: (2, 2, 4),                      # HSG: 2xu16 + VAL4; impl 0x428200
    0xFB: 0,                              # undocumented: no operands (impl 0x42bd80 only resets)
    0x61: (1, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4),  # GPE: u8 + 10xVAL4; impl 0x429510
    0xA0: 0,                              # KIDCLR: no operands (impl 0x42a810 only resets)
    0x87: 0,                              # undocumented: no operands (impl 0x42a680 only resets)
    0xBF: 0,                              # undocumented: no operands (shares impl 0x42ac00; resets)
    0xF3: (4, 4, 4, 4, "s13"),            # AVIP: 4xVAL4 + STR13; impl 0x42b9c0
    0x65: (4, "s13"),                     # GAOPEN: VAL4 + STR13; impl 0x429820
    0x21: (1, 1, "s28"),                  # WP: u8 + u8 + STR28; impl 0x426d10
    0xF5: 1,                              # SVF: u8; impl 0x42bba0
    0x50: (4, 4, 4),                      # VSET: 3xVAL4; impl 0x428ca0
    0xE6: 0,                              # undocumented: no operands (impl 0x42b2d0 only resets)
    0x77: ("s13", 1),                     # PCMON: STR13 + u8; impl 0x42a0c0
    0xE7: 3,                              # undocumented: 3xu8; impl 0x42b2f0
    0x66: (4, 4, 4, 4),                   # GASET: 4xVAL4; impl 0x429870
    0x75: 4,                              # SEP: VAL4; impl 0x429f10
    0x51: (4, 4, 4),                      # GN: 3xVAL4; impl 0x428d00
    0x81: 1,                              # IC: u8; impl 0x42a330
    0x56: (4, "s28"),                     # GL: VAL4 + STR28; impl 0x428f50
    0x06: 0,                              # RT: no operands (sub 0x424b00 touches no stream); impl 0x426210
    0xF9: 5,                              # undocumented: 5xu8; impl 0x42bcd0
    0xE1: 2,                              # undocumented: 2xu8; impl 0x42ad00
    0xF1: 4,                              # ATIMES: VAL4; impl 0x42b990
    0x58: (4, 1, 1, 1, 1, 4, 4, 4, 4),    # GB: VAL4 + 4xu8 + 4xVAL4; impl 0x4292b0
    0xA7: (2, 4),                         # undocumented: u16 + VAL4; impl 0x42ab90
    0xA6: (1, 2, 2),                      # undocumented: u8 + 2xu16; impl 0x42ab20
    0x71: 1,                              # MP: u8; impl 0x429c20
    0xBE: 0,                              # undocumented: no operands (shares reset impl 0x42ac00)
    0x17: 1,                              # CWC: u8; impl 0x426a90
    0x76: 4,                              # SED: VAL4; impl 0x42a050
    0x86: 0,                              # IGINIT: no operands (impl 0x42a660 only resets)
    0x16: (1, 4, 1),                      # CWO: u8 + VAL4 + u8; impl 0x426a10
}

# Ops with a stream-derived counted table: VAL4 (4 bytes, value is
# state-dependent via 0x42c450 so never interpreted) + u8 count + 2*count
# bytes of u16 table. Static length, runtime target. Impls 0x426270/0x4262d0:
# index=VAL4, count=u8, valid iff 0 <= index < count, target = table[index]
# resolved via 0x425870 (scenario-pointer table => state-dependent address).
ONJS_OPS = {0x07, 0x08}                 # ONJP, ONJS

# Ops whose stub calls nothing (ED, HXP) or just the reset (the h135 group: 117 of
# the 139 unassigned ops plus IHK/IHKDEF/PPE/BREAK) take no bytes beyond
# op+prefix. Derived from the EXE bytes, not guessed
# (probe_isf_opcodes.stub_impl_map); the other 22 unassigned ops have real
# impls and stay unverified.
ZEROS = {op for op in range(256)
         if stub_impl_map()[op][1] in (None, 0x424E20)}


def gap_spans(blob):
    """Yield (start, end) of non-text spans (headers, inter-run, trailers).

    Text coverage is 2*len(codes): pairs are 2 bytes each and the lone
    terminator is a 2-byte [code, 0x00] unit. (Every admitted run has one;
    tools/unpack.py --selftest pins it per run.)
    """
    cov = bytearray(len(blob))
    for st, codes in text_runs(blob):
        for j in range(st, st + 2 * len(codes)):
            cov[j] = 1
    i = 0
    while i < len(blob):
        if cov[i]:
            i += 1
            continue
        j = i
        while j < len(blob) and not cov[j]:
            j += 1
        yield i, j
        i = j


def walk_gap(blob, a, b):
    """Parse blob[a:b] as ops. Returns (tiling, stop_or_None).

    tiling = list of (offset, size, op); "TAG" entries are message-tag tails.
    stop is None when every byte is accounted for (ops and/or tag).
    A gap that hits an unverified op, or needs more operand bytes than remain
    beyond a 2-byte tag window, stops there with stop set.

    Message-tag tail: 1-2 bytes that do not complete a KNOWN op, directly
    before the next text marker, are message metadata, not code. Nonzero
    values occur only before quoted (dialogue) lines and cluster per scene
    (DA->04/08, MI->0x0C), while a code reading is impossible: ED(00)
    would fall through into text, JP(04)/ONJS(08) would jump from text
    bytes taken as operands. Consumed as ("TAG", size) tiling entries.
    """
    out = []
    i = a
    n = b
    stop = None
    known = True
    while i < n:
        op_at = i
        op = blob[i]
        i += 1
        if i >= n:
            stop = ("tail-1", op)
            break
        b0 = blob[i]
        i += 1
        if b0 & 0x80:
            if i >= n:
                stop = ("truncated-ext-prefix", op)
                break
            i += 1
        ex = EXTRA.get(op)
        if ex is None:
            if op in ONJS_OPS:
                # VAL4 (4B, value ignored) + u8 count + 2*count table bytes.
                if i + 5 > n:
                    stop = ("truncated-onjs-head", op)
                    break
                count = blob[i + 4]
                size = 5 + 2 * count
                if i + size > n:
                    stop = ("truncated-onjs-table", op)
                    break
                out.append((op_at, (i + size) - op_at, op))
                i += size
                continue
            if op in ZEROS:
                out.append((op_at, i - op_at, op))
                continue
            stop = (f"unverified-op-{op:02X}", op)
            known = False
            break
        if isinstance(ex, int):
            steps = [ex]
        elif ex == "str":
            steps = ["s13"]
        else:
            steps = list(ex)
        good = True
        for st in steps:
            if isinstance(st, int):
                if i + st > n:
                    good = False
                    break
                i += st
            else:
                # The string readers (0x4249c0 cap 13, 0x4249f0 cap 28) consume
                # up to cap bytes and stop early at NUL -- they do NOT require
                # a NUL in the stream. Backend by cap, not by NUL search.
                cap = 13 if st == "s13" else 28
                j = blob.find(b"\x00", i, i + cap)
                i = (j + 1) if j >= 0 else (i + cap)
                if i > n:
                    good = False
                    break
        if not good:
            stop = ("truncated-operands", op)
            break
        out.append((op_at, i - op_at, op))
    if stop is None:
        return out, None
    complete = out[-1][0] + out[-1][1] if out else a
    if known and 1 <= n - complete <= 2 and (blob[n:n + 2] == b"\xff\xfd" or n == len(blob)):
        out.append((complete, n - complete, "TAG"))
        return out, None
    return out, stop


def scan():
    _root, entries, data = read_sm2mpx(SRC)
    files = tiled = blocked = 0
    stops = collections.Counter()
    gaps_tiled = gaps_blocked = gaps_tag = 0
    for name, o, s in entries:
        if not name.endswith(".ISF"):
            continue
        files += 1
        blob = data[o:o + s]
        ok = True
        for a, b in gap_spans(blob):
            if a == 0:
                continue  # file header (u32 tables), not bytecode
            t, stop = walk_gap(blob, a, b)
            if stop is not None:
                ok = False
                stops[stop[0]] += 1
                break
        if ok:
            tiled += 1
        else:
            blocked += 1
    # Per-gap totals (independent of the per-file short-circuit above).
    gaps_tiled = gaps_blocked = gaps_tag = 0
    gap_stops = collections.Counter()
    for name, o, s in entries:
        if not name.endswith(".ISF"):
            continue
        blob = data[o:o + s]
        for a, b in gap_spans(blob):
            if a == 0:
                continue
            t, stop = walk_gap(blob, a, b)
            if stop is None:
                gaps_tiled += 1
                if any(op == "TAG" for _, _, op in t):
                    gaps_tag += 1
            else:
                gaps_blocked += 1
                gap_stops[stop[0]] += 1
    print(f"files={files} fully-tiled={tiled} blocked={blocked}")
    for k, v in stops.most_common(15):
        print(f"  {v:6d}  {k}")
    print(f"gaps tiled={gaps_tiled} (TAG: {gaps_tag}) blocked={gaps_blocked}")
    for k, v in gap_stops.most_common(15):
        print(f"  {v:6d}  {k}")


def val4_desc(raw):
    """Classify a tagged VAL4 per the 0x42c450 decoder (static, not the value)."""
    idx = raw & 0x3FFFFFFF
    flags = []
    if raw & 0x20000000:
        flags.append("SIGN")
    if raw & 0x80000000:
        flags.append("VAR")
    if raw & 0x40000000:
        flags.append("RND")
    tag = "|".join(flags) if flags else "LIT"
    return f"{raw:#010x} idx={idx} [{tag}]"


def disassemble(name):
    """Disassemble one ISF scene end to end (text + gaps + tags = whole file)."""
    _root, entries, data = read_sm2mpx(SRC)
    o, s = next((o, s) for n, o, s in entries if n == name)
    blob = data[o:o + s]
    mntab = op_table()
    files = {n for n, _, _ in entries if n.endswith(".ISF")}

    def mnem(op):
        if op in mntab:
            return mntab[op][0][0]
        if op in ZEROS:
            return "reset"
        return f"impl={stub_impl_map()[op][1]:#x}"

    runs = dict(text_runs(blob))
    print(f"### {name} ({len(blob)} bytes, {len(runs)} text runs)")
    for a, b in gap_spans(blob):
        if a == 0:
            # file header: u32 table, not bytecode (E12)
            words = struct.unpack_from(f"<{min(b, 32) // 4}I", blob, 0)
            print(f"header@{a:#x}..{b:#x}: {' '.join(f'{w:#010x}' for w in words)}")
            continue
        if a in runs:
            pass  # text printed with its following gap below
        t, stop = walk_gap(blob, a, b)
        assert stop is None, f"{name} gap@{a:#x}: {stop}"
        for off, sz, op in t:
            if op == "TAG":
                print(f"  @{off:#06x} TAG {bytes(blob[off:off + sz]).hex(' ')}")
                continue
            p0 = blob[off + 1]
            if p0 & 0x80:
                pre, pn = blob[off + 1:off + 3].hex(" "), ((p0 & 0x7F) << 8) | blob[off + 2]
                i = off + 3
            else:
                pre, pn = f"{p0:02x}", p0
                i = off + 2
            parts = [f"@{off:#06x} {op:02X} {mnem(op):6s} pre[{pre}]={pn}"]
            ex = EXTRA.get(op)
            if op in ONJS_OPS:
                raw, = struct.unpack_from("<I", blob, i)
                count = blob[i + 4]
                tab = struct.unpack_from(f"<{count}H", blob, i + 5) if count else ()
                parts.append(f"VAL4={val4_desc(raw)} count={count} table={[f'{v:#06x}' for v in tab]}")
            elif ex is not None and ex != 0:
                steps = [ex] if isinstance(ex, int) else (["s13"] if ex == "str" else list(ex))
                for st in steps:
                    if isinstance(st, int):
                        parts.append(f"+{st}B={bytes(blob[i:i + st]).hex(' ')}")
                        i += st
                    else:
                        cap = 13 if st == "s13" else 28
                        j = blob.find(b"\x00", i, i + cap)
                        j = (j + 1) if j >= 0 else (i + cap)
                        parts.append(f"{st}={bytes(blob[i:j]).hex(' ')}")
                        i = j
            print(" ".join(parts))
        if b in runs:
            codes = runs[b]
            print(f"  T@{b:#06x} ({len(codes)}): {''.join(char(c) for c in codes)[:90]!r}")
    covered = sum(1 for _ in gap_spans(blob))  # partition check below
    n_text = sum(2 * len(c) for _, c in runs.items())
    print(f"# gaps={covered} text_bytes={n_text} file={len(blob)}")
    assert sum(b - a for a, b in gap_spans(blob)) + n_text == len(blob)


def avip_scan(quiet=False):
    """Every AVIP-shaped byte run in the archive (E19, the movie-path verdict).

    AVIP (op 0xF3, impl 0x42b9c0) reads four 4-byte VALs then a C-string; the file it
    opens is VideoDir + name + ".AVI" (EXE format strings "%s%s", ".AVI"). So a real call
    is F3, a 1- or 2-byte prefix, 16 bytes, then a NUL-terminated name. Byte-level on
    purpose: it does not depend on the static tiling being complete.
    """
    import re
    _root, entries, data = read_sm2mpx(SRC)
    name_re = re.compile(rb"[\x21-\x7e\x81-\xfc]{1,13}\x00")
    hits = []
    for n, o, s in entries:
        blob = data[o:o + s]
        for i in (m.start() for m in re.finditer(rb"\xf3", blob)):
            for pre in (1, 2):
                m = name_re.match(blob, i + 1 + pre + 16)
                if m:
                    hits.append((n, i, pre, m.group()[:-1]))
    plausible = sum(1 for h in hits if re.fullmatch(rb"[A-Za-z0-9_-]{2,13}", h[3]))
    if not quiet:
        print(f"# entries={len(entries)} avip_shaped={len(hits)} plausible_names={plausible}")
        for n, i, pre, name in hits:
            print(f"{n} @{i:#06x} prefix={pre} name={name!r}")
    return len(hits), plausible


if __name__ == "__main__":
    if "--avip" in sys.argv:
        avip_scan()
    elif "--scan" in sys.argv:
        scan()
    elif "--scene" in sys.argv:
        disassemble(sys.argv[sys.argv.index("--scene") + 1])
    else:
        _root, entries, data = read_sm2mpx(SRC)
        name, o, s = next((n, o, s) for n, o, s in entries if n == "A0_0.ISF")
        blob = data[o:o + s]
        for a, b in list(gap_spans(blob)):
            if a == 0:
                continue  # file header, not bytecode
            t, stop = walk_gap(blob, a, b)
            print(f"gap@{a:#x}..{b:#x} ops={len(t)} stop={stop} "
                  f"bytes={blob[a:a + 16].hex(' ')}")
            if a > 0x160:
                break
