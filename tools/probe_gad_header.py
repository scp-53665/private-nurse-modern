"""Parse the GAD animation-sheet container header (U6 groundwork).

Layout, verified by assertion over all four .GAD entries (OBSERVED):
  +0x00 u8[4]  magic "GAD "
  +0x04 u32    absolute payload offset of the B9 AA B3 B3 image tag (self-describing:
               278 for ANGEL/ANIME/CDF08, 86 for ANGEL3 -- NOT a constant 278)
  +0x08 u32    image count, always 1 (consistent with exactly one tag per entry)
  +0x0C u16[4] prologue (per-sheet; meaning UNKNOWN)
  +0x14 N * { u16[4] } frame records, then one trailing u16 zero word

Frame records (OBSERVED): record 0 of each group is (S,0,0,150); the rest are
(0,BASE,k*S,150) for k = 1..7, where S = 0x5a (ANGEL/ANIME/ANGEL3) or 0x28 (CDF08)
and BASE is a per-group u16: 0x0/0x168/0x2d0/0x438 in ANGEL, 0x0/0x169/0x2d1/0x0
in ANIME, always 0x0 in CDF08/ANGEL3. Groups hold 8 records; ANGEL/ANIME have 4
groups (32 records), CDF08 has 32 records in one BASE=0 group, ANGEL3 has 8.
The (x,y,w,h)-as-rect reading fits records 1..7 but not record 0 (w=0), and the
BASE word (360/720/1080 -- suspiciously 360*n, vs 361/721/0 in ANIME) is UNKNOWN:
y-offset, delay and flags are all live candidates. Semantics belong with U1/U2.
"""
import struct
from pathlib import Path

PN = Path(__file__).resolve().parent.parent / "original"
TAG = b"\xb9\xaa\xb3\xb3"


def gad_entries(path):
    d = path.read_bytes()
    (count,) = struct.unpack_from("<I", d, 8)
    for i in range(count):
        off = 0x20 + i * 20
        n = d[off:off + 12].rstrip(b"\0").decode("latin1")
        if not n.lower().endswith(".gad"):
            continue
        o, s = struct.unpack_from("<II", d, off + 12)
        yield n, d[o:o + s]


def parse(blob):
    assert blob[:4] == b"GAD ", blob[:4]
    (tagoff, imgcount) = struct.unpack_from("<II", blob, 4)
    assert blob[tagoff:tagoff + 4] == TAG, (tagoff, blob[tagoff:tagoff + 4].hex())
    hdr = blob[4:tagoff]
    prologue = struct.unpack_from("<HHHH", hdr, 8)
    rest = hdr[16:]
    words = struct.unpack_from("<%dH" % (len(rest) // 2), rest)
    assert words[-1] == 0, words[-3:]  # trailing zero word
    body = words[:-1]
    assert len(body) % 4 == 0, len(body)
    recs = [body[i:i + 4] for i in range(0, len(body), 4)]
    w, h = struct.unpack_from("<HH", blob, tagoff + 4)
    return {"tagoff": tagoff, "imgcount": imgcount, "prologue": prologue,
            "recs": recs, "dims": (w, h)}


def main():
    parsed = {}
    for n, blob in gad_entries(PN / "GGD"):
        p = parse(blob)
        parsed[n] = p
        print(f"{n}: tagoff={p['tagoff']} imgcount={p['imgcount']} "
              f"prologue={[hex(v) for v in p['prologue']]} "
              f"frames={len(p['recs'])} dims={p['dims'][0]}x{p['dims'][1]}")
        # group boundaries: the BASE word (rec[1]) changes. No size assumption --
        # CDF08 never changes BASE and is one group of 32; ANGEL changes every 8.
        groups, start = [], 0
        for i in range(1, len(p["recs"])):
            if p["recs"][i][1] != p["recs"][start][1]:
                groups.append((start, i))
                start = i
        groups.append((start, len(p["recs"])))
        for a, b in groups:
            r0, r1, rN = p["recs"][a], p["recs"][a + 1], p["recs"][b - 1]
            print(f"   group base={r1[1]:#x} recs {a}..{b - 1}: "
                  f"r0={tuple(hex(v) for v in r0)} "
                  f"r1={tuple(hex(v) for v in r1)} "
                  f"rN={tuple(hex(v) for v in rN)}")
    names = sorted(parsed)
    print("record-count:", {n: len(parsed[n]["recs"]) for n in names})
    a3, an = parsed["ANGEL3.GAD"]["recs"], parsed["ANGEL.GAD"]["recs"]
    print("ANGEL3 recs == ANGEL recs[0:8]:", a3 == an[:8])
    am = parsed["ANIME.GAD"]["recs"]
    assert len(am) == len(an)
    diffs = [(i, y, x) for i, (x, y) in enumerate(zip(am, an)) if x != y]
    print(f"ANIME vs ANGEL: {len(diffs)}/{len(an)} frame records differ")
    for i, old, new in diffs[:4] + diffs[-2:]:
        print(f"   rec{i}: ANGEL={tuple(hex(v) for v in old)} "
              f"ANIME={tuple(hex(v) for v in new)}")


if __name__ == "__main__":
    main()
