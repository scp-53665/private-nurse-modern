"""GGD pixel decoder (U1). Grammar recovered from NURSEUS.EXE disassembly
(op dispatch at 0x40827d, jump table 0x408438, cases 0x408292-0x4083e0):

  0x00 NN          run of NN copies of the previous triplet (NN=0: no-op)
  0x01 LL OO       copy LL triplets from OO triplets back
  0x02 LL OO OO   copy LL triplets from OOOO (u16LE) triplets back
  0x03 OO          copy 1 triplet from OO triplets back
  0x04 OO OO       copy 1 triplet from OOOO (u16LE) triplets back
  0x05..0xFF       (op-4) literal RGB triplets
Decode stops at w*h pixels (trailing bytes, if any, are ignored).
"""
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "original"
TAG = b"\xb9\xaa\xb3\xb3"


def entries(path):
    d = path.read_bytes()
    (count,) = struct.unpack_from("<I", d, 8)
    for i in range(count):
        off = 0x20 + i * 20
        n = d[off:off + 12].split(b"\0")[0].decode("latin1")
        o, s = struct.unpack_from("<II", d, off + 12)
        yield n, d[o:o + s]


def decode(blob, base=0):
    """Returns (w, h, pixels bytes RGBRGB...). base = tag offset in blob."""
    w, h = struct.unpack_from("<HH", blob, base + 4)
    N = w * h * 3
    out = bytearray()
    n = len(blob)
    i = base + 8
    while len(out) < N:
        if i >= n:
            raise ValueError("stream exhausted")
        op = blob[i]
        i += 1
        if op == 0x00:
            cnt = blob[i]
            i += 1
            out += bytes(out[-3:]) * cnt
        elif op in (0x01, 0x02):
            if op == 0x01:
                ln, off = blob[i], blob[i + 1]
                i += 2
            else:
                ln = blob[i]
                off = blob[i + 1] | (blob[i + 2] << 8)
                i += 3
            base = len(out) - off * 3
            need = ln * 3
            if base < 0:
                out += b"\x00" * min(need, -base)
                base = 0
                need = ln * 3 - min(ln * 3, -base)
            if need:
                if off * 3 >= need:
                    out += out[base:base + need]
                else:
                    pat = bytes(out[base:base + off * 3]) or b"\x00" * 3
                    out += (pat * ((need // len(pat)) + 1))[:need]
        elif op in (0x03, 0x04):
            if op == 0x03:
                off = blob[i]
                i += 1
            else:
                off = blob[i] | (blob[i + 1] << 8)
                i += 2
            base = len(out) - off * 3
            for k in range(3):
                s = base + k
                out.append(out[s] if 0 <= s < len(out) else 0)
        else:
            k = op - 4
            out += blob[i:i + 3 * k]
            i += 3 * k
    return w, h, bytes(out[:N])


def scan():
    bad = 0
    total = 0
    for name, b in entries(SRC / "GGD"):
        total += 1
        try:
            base = b.find(TAG)
            assert base >= 0, "no tag"
            w, h, px = decode(b, base)
            assert len(px) == w * h * 3
        except Exception as e:
            bad += 1
            if bad <= 10:
                print(f"  FAIL {name}: {e}")
    print(f"decoded={total - bad}/{total}")
    assert bad == 0, f"{bad} entries failed to decode"


def save_png(name, outpath):
    from PIL import Image
    b = dict(entries(SRC / "GGD"))[name]
    w, h, px = decode(b, b.find(TAG))
    Image.frombuffer("RGB", (w, h), px, "raw", "RGB", 0, 1).save(outpath)
    print(f"{name} {w}x{h} -> {outpath}")


if __name__ == "__main__":
    if "--scan" in sys.argv or "--selftest" in sys.argv:
        scan()
    elif len(sys.argv) == 4 and sys.argv[1] == "--png":
        save_png(sys.argv[2], sys.argv[3])
    else:
        d = dict(entries(SRC / "GGD"))
        for name in ("BLACK.GGD", "WHITE.GGD"):
            w, h, px = decode(d[name])
            cols = set(px[i:i + 3] for i in range(0, len(px), 3))
            print(f"{name} {w}x{h} px={len(px) // 3} colors={len(cols)}")
            assert len(px) == w * h * 3 and len(cols) == 1
        print("selftest: PASS")
