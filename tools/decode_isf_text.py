"""ISF dialogue decoder.

Encoding, established by frequency analysis (see docs/evidence/E04, derivation in
docs/02-formats.md 4.1):

  A dialogue stream is a byte stream of <code> 0xFD pairs -- NOT a 16-bit aligned
  word stream; runs begin at arbitrary byte offsets, and roughly half of all 0xFD
  bytes sit at odd offsets within their entry.

  code 0xFF          -> 0xFDFF, the run/message marker (never appears inside a run)
  code = 4*(a & 0x3F) | class
      class 1 (code & 3 == 1) -> letter: x = code >> 2, 1..26 -> 'A'..'Z',
                                                  33..58 -> 'a'..'z'
      class 0 (code & 3 == 0) -> symbol: chr(code >> 2) when 0x20 <= x < 0x40
      class 2 / 3            -> never occur as characters

  Every message (20115/20115) ends with one UNPAIRED code byte followed by
  0x00: the final punctuation or letter, carried without its 0xFD marker.
  Without it the corpus has 7030 '"' and no closer, and lines end mid-word.
  text_runs() appends that lone code to the run by default.

  text_runs(min_len=3) drops 186 one-pair pseudo-runs (a bare `+` code 0xAC
  immediately before a run marker, i.e. a bytecode artefact, not dialogue). With
  min_len=1 every 0xFD byte in the ISF payload region is accounted for exactly;
  tools/unpack.py --selftest asserts that balance.
"""
import collections
import struct
import sys
from pathlib import Path

PN = Path(__file__).resolve().parent.parent / "original"
MARK = 0xFD


def entries(path):
    d = path.read_bytes()
    (count,) = struct.unpack_from("<I", d, 8)
    for i in range(count):
        off = 0x20 + i * 20
        n = d[off:off + 12].rstrip(b"\0").decode("latin1")
        o, s = struct.unpack_from("<II", d, off + 12)
        yield n, d[o:o + s]


def char(code):
    """code = 4*(ascii & 0x3F) | class, class 1 = letter, class 0 = symbol."""
    x = code >> 2
    if code & 3 == 1:
        if 1 <= x <= 26:
            return chr(64 + x)          # A-Z
        if 33 <= x <= 58:
            return chr(x + 64)          # a-z
    elif code & 3 == 0:
        if 0x20 <= x < 0x40:
            return chr(x)               # space, punctuation, digits
    return f"{{{code:02X}}}"


def text_runs(blob, min_len=3, lone=True):
    """Yield (offset, codes) for runs of <code>0xFD pairs.

    With lone=True (default), a run followed by [valid-code][0x00] gains that
    code as its final character: the last character's second byte is 0x00, not
    0xFD. Coverage-wise the lone occupies 2 stream bytes, so walkers can treat
    2*len(codes) as the text span. With lone=False the raw pair runs are
    returned (used for the 0xFD budget).
    """
    runs = []
    i = 0
    while i < len(blob) - 1:
        if blob[i + 1] == MARK:
            codes = []
            start = i
            while i < len(blob) - 1 and blob[i + 1] == MARK:
                codes.append(blob[i])
                i += 2
            if (lone and i < len(blob) - 1 and blob[i + 1] == 0x00
                    and not char(blob[i]).startswith("{")):
                codes.append(blob[i])
            if len(codes) >= min_len:
                runs.append((start, codes))
            continue
        i += 1
    return runs


def all_lines():
    """Yield (entry, offset, text) for every decoded message line in ISF."""
    for n, blob in entries(PN / "ISF"):
        for off, codes in text_runs(blob):
            yield n, off, "".join(char(c) for c in codes)


def main():
    if len(sys.argv) > 2 and sys.argv[1] == "--dump":
        out = Path(sys.argv[2])
        out.parent.mkdir(parents=True, exist_ok=True)
        lines = 0
        with out.open("w", encoding="utf-8", newline="\n") as fh:
            cur = None
            for n, off, text in all_lines():
                if n != cur:
                    fh.write(f"\n##### {n}\n")
                    cur = n
                fh.write(text.replace("{FF}", "") + "\n")
                lines += 1
        print(f"wrote {lines} dialogue lines to {out}")
        return

    unknown = collections.Counter()
    total_chars = 0
    leading_ff = 0
    ff_inside = 0
    lone_n = 0
    lone_c = collections.Counter()
    even_fd = odd_fd = 0       # 0xFD bytes by entry-relative offset parity
    even_mark = odd_mark = 0   # run-marker FF bytes by entry-relative parity
    allcodes = set()
    for n, blob in entries(PN / "ISF"):
        for i, b in enumerate(blob):
            if b == MARK:
                if i % 2 == 0:
                    even_fd += 1
                else:
                    odd_fd += 1
        raw = dict(text_runs(blob, min_len=1, lone=False))
        for _off, codes in text_runs(blob):
            total_chars += len(codes)
            leading_ff += sum(1 for c in codes[:1] if c == 0xFF)
            ff_inside += sum(1 for c in codes[1:] if c == 0xFF)
            if len(codes) == len(raw[_off]) + 1:
                lone_n += 1
                lone_c[char(codes[-1])] += 1
            if codes and codes[0] == 0xFF:
                if _off % 2 == 0:
                    even_mark += 1
                else:
                    odd_mark += 1
            for c in codes:
                allcodes.add(c)
                if char(c).startswith("{"):
                    unknown[c] += 1
    print(f"decoded {total_chars} dialogue characters across ISF")
    print(f"0xFF occurrences: {leading_ff} as first char of a run, {ff_inside} inside runs")
    print(f"lone terminators appended: {lone_n} runs, "
          + ", ".join(f"{c!r}x{v}" for c, v in lone_c.most_common(8)))
    print(f"0xFD bytes by entry-relative offset parity: even={even_fd} odd={odd_fd}")
    print(f"run markers (leading 0xFF) by entry-relative parity: "
          f"even={even_mark} odd={odd_mark}")
    print(f"distinct character codes in decoded runs: {len(allcodes)}")
    print("unmapped codes by frequency:",
          [(f"{c:02X}", v) for c, v in unknown.most_common(20)])
    print()
    # Offsets and lengths only: the decoded text belongs to the game and is never
    # written to docs/evidence. Use --dump for a local copy.
    for n, blob in entries(PN / "ISF"):
        if n in ("A0_0.ISF", "A0_1.ISF", "A0_2.ISF", "GAME.ISF"):
            for off, codes in text_runs(blob)[:4]:
                print(f"--- {n} @0x{off:x} ({len(codes)} chars)")


if __name__ == "__main__":
    main()
