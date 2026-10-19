"""Determine bits per pixel of GGD/GAD image payloads by size ratio."""
import collections
import struct
from pathlib import Path

PN = Path(__file__).resolve().parent.parent / "original"
TAG = b"\xb9\xaa\xb3\xb3"


def rows(path):
    """(ratio, name, pixel_bytes, w, h, tag_offset) per entry carrying a tag."""
    d = path.read_bytes()
    (count,) = struct.unpack_from("<I", d, 8)
    out = []
    for i in range(count):
        off = 0x20 + i * 20
        n = d[off:off + 12].rstrip(b"\0").decode("latin1")
        o, s = struct.unpack_from("<II", d, off + 12)
        blob = d[o:o + s]
        p = blob.find(TAG)
        if p < 0:
            continue
        w, h = struct.unpack_from("<HH", blob, p + 4)
        if w and h:
            # ratio is measured against the pixel bytes only, header excluded
            out.append((round((s - p - 8) / (w * h), 3), n, s, w, h, p))
    return out


for f in ("GGD",):
    rs = rows(PN / f)
    hist = collections.Counter(r[0] for r in rs)
    print(f"{f}: {len(rs)} entries with a tag; ratio (payload-tag-header)/(w*h):")
    for k, v in hist.most_common(12):
        print(f"   ratio {k:<7} count {v}")
    print("   tag offsets present:", sorted(set(r[5] for r in rs)),
          "(0 = GGD-style, 278 = GAD w/ 0x116 header)")
    def band(lo, hi):
        return sum(v for k, v in hist.items() if lo <= k <= hi)
    print(f"   bands: 0.46-0.50 -> {band(0.46, 0.50)} | 1.93-2.18 -> {band(1.93, 2.18)}"
          f" | 3.08-3.13 -> {band(3.08, 3.13)}")
    # NB: an earlier revision printed "~3 => 24bpp" here. That heuristic is wrong as a
    # bound: ratio > 3 only rules out a *bare* 24bpp raw buffer, not 24bpp + overhead.
    # The header is 8 bytes (negligible), so >=3.08 implies a raw form of >3 B/px.
    print("   smallest entries:", sorted(rs)[:5])
    print("   largest entries:", sorted(rs, reverse=True)[:3])
    mx = max(rs)
    print(f"   worst case {mx[1]}: {mx[2]} B at {mx[3]}x{mx[4]}")
    print(f"     raw 24bpp = {mx[3] * mx[4] * 3} -> overhead would need to be "
          f"{mx[2] - mx[3] * mx[4] * 3} B ({(mx[2] - mx[3] * mx[4] * 3) / mx[2]:.2%} of payload)")
    print(f"     raw 32bpp = {mx[3] * mx[4] * 4} -> codec ratio {mx[2] / (mx[3] * mx[4] * 4):.3f}")
