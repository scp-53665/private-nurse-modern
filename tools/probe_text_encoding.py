"""Test the hypothesis that ISF scenario text is a 16-bit stream with a fixed high byte.

Method: for every ISF entry, walk aligned u16s; if a large fraction share one high
byte H, that byte is a marker/escape and the low byte carries the payload. Then
compare the payload distribution against English letter frequency.
"""
import collections
import struct
from pathlib import Path

PN = Path(__file__).resolve().parent.parent / "original"
EN = " etaoinshrdlucmfwypvbgkjqxz"  # rough English frequency order (desc)


def entries(path):
    d = path.read_bytes()
    (count,) = struct.unpack_from("<I", d, 8)
    out = []
    for i in range(count):
        off = 0x20 + i * 20
        n = d[off:off + 12].rstrip(b"\0").decode("latin1")
        o, s = struct.unpack_from("<II", d, off + 12)
        out.append((n, d[o:o + s]))
    return out


ents = entries(PN / "ISF")
hi = collections.Counter()
total = 0
for n, b in ents:
    vals = struct.unpack_from("<%dH" % (len(b) // 2), b, 0)
    for v in vals:
        hi[v >> 8] += 1
        total += 1
print("total aligned u16 across ISF entries:", total)
print("top high bytes:", [(hex(k), v, f"{v / total:.1%}") for k, v in hi.most_common(8)])

H = hi.most_common(1)[0][0]
lows = collections.Counter()
for n, b in ents:
    vals = struct.unpack_from("<%dH" % (len(b) // 2), b, 0)
    for v in vals:
        if v >> 8 == H:
            lows[v & 0xFF] += 1
print(f"\nmarker high byte = {H:#x}; payload bytes = {sum(lows.values())} distinct={len(lows)}")
top = lows.most_common(24)
print("payload histogram (top 24), as code:count -- the doc cites these codes:")
print("  ", " ".join(f"{k:#04x}:{v}" for k, v in top))

# does the payload look like text shifted by a constant?
print("\nbest constant-shift match to English letter frequency:")
for shift in range(0, 256, 1):
    mapped = collections.Counter()
    for k, v in lows.items():
        c = (k - shift) & 0xFF
        if 0x20 <= c < 0x7F:
            mapped[chr(c).lower()] += v
    top20 = [c for c, _ in mapped.most_common(20)]
    score = sum(1 for i, c in enumerate(top20) if c in EN[:20])
    if score >= 16:
        print(f"  shift {shift:#x} score {score}: {''.join(top20)}  ({sum(mapped.values())} chars in ascii)")
