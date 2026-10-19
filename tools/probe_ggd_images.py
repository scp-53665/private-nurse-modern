"""Probe inner GGD/GAD image payloads.

Observation: GGD container entries start with b'\\xb9\\xaa\\xb3\\xb3' + u16 width + u16 height.
`.GAD` entries instead begin with the ASCII magic b'GAD ' followed by a header whose
length varies (278 for ANGEL/ANIME/CDF08, 86 for ANGEL3), so the tag offset is a
per-entry field, not a constant. See docs/02-formats.md 3.
"""
import collections
import struct
from pathlib import Path

PN = Path(__file__).resolve().parent.parent / "original"
TAG = b"\xb9\xaa\xb3\xb3"


def entries(path):
    d = path.read_bytes()
    (count,) = struct.unpack_from("<I", d, 8)
    for i in range(count):
        off = 0x20 + i * 20
        n = d[off:off + 12].rstrip(b"\0").decode("latin1")
        o, s = struct.unpack_from("<II", d, off + 12)
        yield n, d[o:o + s]


def probe(path):
    print(f"=== {path.name}")
    tagged = collections.Counter()
    offsets = collections.Counter()
    gad_magic = collections.Counter()
    shown = 0
    for n, blob in entries(path):
        hits = []
        i = blob.find(TAG)
        while i >= 0:
            w, h = struct.unpack_from("<HH", blob, i + 4)
            hits.append((i, w, h))
            i = blob.find(TAG, i + 1)
        if not hits:
            continue
        tagged[len(hits)] += 1
        offsets[hits[0][0]] += 1
        if n.lower().endswith(".gad"):
            gad_magic[blob[:4]] += 1
        if shown < 8:
            print(f"  {n}: size={len(blob)} images={len(hits)} first_tag={hits[0][0]} "
                  f"dims={[f'{w}x{h}' for _, w, h in hits[:8]]} magic={blob[:4]!r}")
        shown += 1
    print(f"  -- entries carrying a tag: {sum(tagged.values())}")
    print(f"  -- tag-count histogram: {dict(tagged)}  (1 = exactly one tag per entry)")
    print(f"  -- first-tag-offset histogram: {dict(sorted(offsets.items()))}")
    print(f"  -- .GAD leading magic: {dict(gad_magic)}  (b'GAD ' container header)")


if __name__ == "__main__":
    probe(PN / "GGD")
