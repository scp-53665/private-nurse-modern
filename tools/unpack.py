"""Verified extractor for the Private Nurse (AngelSmile, 2002) data files.

Formats (see docs/02-formats.md for the derivation and evidence):

  SM2MPX10 container (GGD, ISF)
      0x00 u8[8]  b"SM2MPX10"
      0x08 u32    entry count
      0x0C u32    data start (== 0x20 + count*20)
      0x10 u8[12] root name ("GGD" / "ISF")
      0x1C u32    table offset (0x20)
      0x20 count * { u8[12] name; u32 offset; u32 size }
      payloads, each aligned up to 16 bytes

  SE / VOICE  (audio archives)
      0x00 u16    table size in bytes
      0x02 ..     table size/16 * { u8[12] name; u32 size }
      payloads sequential, no padding

  WMSC (single WAV)
      0x00 u16    table size
      0x02 ..     table size/32 * { u8[12] name; u32 offset; u8[12] zero; u32 end_offset }
                  (end_offset == filesize; this is the only EOF pin WMSC has, since its
                  last record is named rather than empty)

Usage:
    python tools/unpack.py [--out DIR] [--selftest] [files...]
"""
import argparse
import collections
import struct
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "original"
MAGIC = b"SM2MPX10"
TAG = b"\xb9\xaa\xb3\xb3"


def read_sm2mpx(path, check=True):
    d = path.read_bytes()
    assert d[:8] == MAGIC, f"{path.name}: not SM2MPX10 ({d[:8]!r})"
    count, data_start = struct.unpack_from("<II", d, 8)
    root = d[0x10:0x1C].rstrip(b"\0").decode("latin1")
    (table_off,) = struct.unpack_from("<I", d, 0x1C)
    if check:
        assert table_off == 0x20, f"{path.name}: table offset {table_off:#x}"
        assert data_start == 0x20 + count * 20, f"{path.name}: data start mismatch"
    entries = []
    for i in range(count):
        off = table_off + i * 20
        name = d[off:off + 12].rstrip(b"\0").decode("latin1")
        o, s = struct.unpack_from("<II", d, off + 12)
        assert o + s <= len(d), f"{path.name}: entry {name} past EOF"
        entries.append((name, o, s))
    if check:
        # The first payload is the table end rounded up to the 16-byte payload
        # alignment -- GGD needs 0 pad (0x5110), ISF needs 4 (0x3edc -> 0x3ee0).
        # Nothing in the file states this; without the assert a shifted first offset
        # silently mis-slices that entry.
        want0 = (data_start + 15) & ~15
        assert entries[0][1] == want0, (
            f"{path.name}: first payload at {entries[0][1]:#x} != align16(data start) {want0:#x}")
        assert d[data_start:entries[0][1]] == b"\0" * (entries[0][1] - data_start), \
            f"{path.name}: pre-payload pad is not zero"
        for (n1, o1, s1), (n2, o2, _s2) in zip(entries, entries[1:]):
            padded = (o1 + s1 + 15) & ~15
            assert padded == o2, f"{path.name}: gap after {n1} (0x{padded:x} != 0x{o2:x})"
        last = entries[-1]
        assert len(d) - (last[1] + last[2]) < 16, f"{path.name}: trailing bytes unaccounted"
    return root, entries, d


def read_table(path, rec_size, check=True):
    """SE/VOICE (rec 16) and WMSC (rec 32).

    Layout: u16 table size; then records of {char name[12]; u32 start; ...}.
    Names are absolute start offsets into the file and are stored per file, with a
    final record whose name is empty marking EOF (SE/VOICE).  A file's size is the
    next record's start (or EOF).

    The interior offsets are pinned by WAV payload magic, not by the offset arithmetic:
    `sum(sizes) == filesize - table_end` holds by construction under any *interior*
    shift (a shift only moves bytes between two adjacent files), so it catches nothing.
    """
    d = path.read_bytes()
    (table_size,) = struct.unpack_from("<H", d, 0)
    assert table_size % rec_size == 0, f"{path.name}: table size {table_size} % {rec_size}"
    n = table_size // rec_size
    recs = []
    for i in range(n):
        rec = 2 + i * rec_size
        name = d[rec:rec + 12].rstrip(b"\0").decode("latin1")
        (start,) = struct.unpack_from("<I", d, rec + 12)
        recs.append((name, start))
    if check and recs:
        assert recs[0][1] == 2 + table_size, f"{path.name}: first start {recs[0][1]:#x} != table end"
        assert all(a[1] < b[1] for a, b in zip(recs, recs[1:])), \
            f"{path.name}: starts not strictly increasing (zero-length or reordered file)"
        if rec_size == 32:
            assert d[2 + 16:2 + 28] == b"\0" * 12, f"{path.name}: record bytes 16..27 not zero"
            (end_off,) = struct.unpack_from("<I", d, 2 + 28)
            assert end_off == len(d), f"{path.name}: end_offset {end_off:#x} != filesize {len(d):#x}"
    # EOF marker: only present when the last record is unnamed (SE/VOICE, not WMSC).
    if check and recs and not recs[-1][0]:
        assert recs[-1][1] == len(d), \
            f"{path.name}: eof marker {recs[-1][1]:#x} != filesize {len(d):#x}"
    eof = recs[-1][1] if recs and not recs[-1][0] else len(d)
    entries = []
    for i, (name, start) in enumerate(recs):
        if not name:
            continue
        end = recs[i + 1][1] if i + 1 < len(recs) else eof
        entries.append((name, start, end - start))
    if check:
        # interior-offset pin: every payload is a real RIFF/WAVE, so a shifted interior
        # start (which leaves the size arithmetic intact) is caught here.
        for name, off, size in entries:
            assert size >= 44 and d[off:off + 4] == b"RIFF" and d[off + 8:off + 12] == b"WAVE", \
                f"{path.name}: {name} at {off:#x} (+{size} B) is not a RIFF/WAVE payload"
    return entries, d


def read_seq_table(path, check=True):
    return read_table(path, 16, check)


def read_wmsc(path, check=True):
    return read_table(path, 32, check)


def readers(path):
    """Return (entries, data) for any archive. Normalised here so callers never have
    to know that SM2MPX also yields a root name."""
    head = path.read_bytes()[:8]
    if head == MAGIC:
        _root, entries, data = read_sm2mpx(path)
        return entries, data
    if path.name == "WMSC":
        return read_wmsc(path)
    return read_seq_table(path)


def extract(path, out_root, quiet=False):
    entries, d = readers(path)
    out = out_root / path.name
    out.mkdir(parents=True, exist_ok=True)
    total = 0
    for name, off, size in entries:
        safe = name.replace("/", "_").replace("\\", "_")
        (out / safe).write_bytes(d[off:off + size])
        total += size
    if not quiet:   # quiet keeps selftest stdout deterministic (temp paths vary per run)
        print(f"{path.name}: {len(entries)} entries, {total} bytes -> {out}")
    return entries


def selftest():
    """Fails loudly if any format assumption or decoder invariant breaks."""
    ggd_root, ggd, _ = read_sm2mpx(SRC / "GGD")
    isf_root, isf, isf_data = read_sm2mpx(SRC / "ISF")
    assert (ggd_root, isf_root) == ("GGD", "ISF"), (ggd_root, isf_root)
    assert len(ggd) == 1036 and len(isf) == 803, (len(ggd), len(isf))

    se, _ = read_seq_table(SRC / "SE")
    voice, _ = read_seq_table(SRC / "VOICE")
    assert len(se) == 77, len(se)
    assert len(voice) == 2715, len(voice)

    wmsc, _ = read_wmsc(SRC / "WMSC")
    assert [n for n, _o, _s in wmsc] == ["MUON.WAV"], wmsc

    for label, ents in (("GGD", ggd), ("ISF", isf), ("SE", se), ("VOICE", voice), ("WMSC", wmsc)):
        total = sum(s for _n, _o, s in ents)
        print(f"  {label:<6} entries={len(ents):<5} payload_bytes={total:<12} "
              f"first={ents[0][0]!r} last={ents[-1][0]!r}")

    # inner image tag
    after = dict((n, (o, s)) for n, o, s in ggd)["AFTER.GGD"]
    ggd_data = (SRC / "GGD").read_bytes()
    payload = ggd_data[after[0]:after[0] + after[1]]
    assert payload[:4] == TAG, payload[:4]
    assert struct.unpack_from("<HH", payload, 4) == (640, 480)

    # table geometry both readers assert on (docs/02-formats.md 1-2 cites these)
    gds, ids = struct.unpack_from("<I", ggd_data, 12)[0], struct.unpack_from("<I", isf_data, 12)[0]
    print(f"  SM2MPX data_start: GGD={gds:#x} ISF={ids:#x} "
          f"(== 0x20 + count*20 in both cases)")
    for fname, rec in (("SE", 16), ("VOICE", 16), ("WMSC", 32)):
        d = (SRC / fname).read_bytes()
        (ts,) = struct.unpack_from("<H", d, 0)
        print(f"  {fname}: table_size={ts} records={ts // rec} rec_size={rec}")

    # dialogue decoder. The code->char map is checked against synthetic codes only:
    # the decoded text is the game's own material and is never stored in this repo.
    # The pin below is structural (offset and run length), which catches a
    # segmentation or boundary regression just as well.
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import decode_isf_text as dec

    def encode(ch):
        """Inverse of dec.char(): code = 4*(ascii & 0x3F) | class."""
        return 4 * (ord(ch) & 0x3F) | (1 if ch.isalpha() else 0)

    sample = "Az 09.?!"
    assert "".join(dec.char(encode(c)) for c in sample) == sample, "char() round trip"
    assert dec.char(0xFF) == "{FF}" and dec.char(0x03) == "{03}", "marker / bad class"

    by_name = dict((n, blob) for n, blob in dec.entries(SRC / "ISF"))
    pins = {
        ("A0_0.ISF", 0x22): 19,      # first message of the first script
        ("A0_0.ISF", 0x55): 86,      # long line: exercises class-0 punctuation
        ("A0_0.ISF", 0x10d): 38,
        ("A0_2.ISF", 0x159): 49,
    }
    for (name, off), want_len in pins.items():
        runs = dict(dec.text_runs(by_name[name]))
        assert off in runs, f"{name}@0x{off:x}: run missing"
        assert len(runs[off]) == want_len, \
            f"{name}@0x{off:x}: {len(runs[off])} chars != {want_len}"

    # Corpus-wide invariants (docs/02-formats.md 4.1).
    total_chars = total_runs = ff_lead = ff_inside = quotes = 0
    unmapped = collections.Counter()
    odd_class = collections.Counter()
    for name, blob in by_name.items():
        runs = dec.text_runs(blob)
        raw = dict(dec.text_runs(blob, min_len=1, lone=False))
        for off, codes in runs:
            total_runs += 1
            total_chars += len(codes)
            ff_lead += sum(1 for c in codes[:1] if c == 0xFF)
            ff_inside += sum(1 for c in codes[1:] if c == 0xFF)
            quotes += "".join(dec.char(c) for c in codes).count('"')
            for c in codes:
                if c & 3 in (2, 3):
                    odd_class[c] += 1
                if dec.char(c).startswith("{"):
                    unmapped[c] += 1
            # Segmentation pin: every admitted run is followed by its lone
            # terminator ([valid-code][0x00]), appended as codes[-1].
            end = off + 2 * len(raw[off])
            assert end < len(blob) - 1, f"{name}@0x{off:x}: run at EOF"
            assert blob[end] == codes[-1] and blob[end + 1] == 0, \
                f"{name}@0x{off:x}: lone terminator mismatch"
            assert not dec.char(blob[end]).startswith("{"), \
                f"{name}@0x{off:x}: lone terminator is not a text code"
    assert (total_runs, total_chars) == (20115, 737989), (total_runs, total_chars)
    assert ff_lead == total_runs and ff_inside == 0, (ff_lead, ff_inside)
    assert dict(odd_class) == {0xFF: 20115}, dict(odd_class)
    assert dict(unmapped) == {0xFF: 20115}, dict(unmapped)
    assert quotes == 13897, quotes

    # Every 0xFD byte inside the payload region is accounted for by the <code>0xFD
    # pairing, with no filter applied. (The entry table contributes 9 more 0xFD bytes --
    # e.g. ISF entry #323 has size 0x5FD -- which are offset/size fields, not text.)
    table_end = 0x20 + len(isf) * 20
    unfiltered = sum(len(codes) for blob in by_name.values()
                     for _off, codes in dec.text_runs(blob, min_len=1, lone=False))
    assert isf_data[table_end:].count(0xFD) == unfiltered, \
        f"0xFD budget: {isf_data[table_end:].count(0xFD)} bytes vs {unfiltered} decoded"

    # The write path, not just the readers: extract() used to crash on the SM2MPX
    # archives and no pin noticed, because selftest only ever called the readers.
    # Every archive is extracted to confirm the file count; ISF/SE/WMSC (13 MB) are
    # additionally compared byte-for-byte. GGD+VOICE are 480 MB and share the same
    # write loop, so they are checked by count only.
    count_only = {"GGD", "VOICE"}
    all_ents = {"GGD": ggd, "ISF": isf, "SE": se, "VOICE": voice, "WMSC": wmsc}
    with tempfile.TemporaryDirectory() as td:
        for label, ents in all_ents.items():
            extract(SRC / label, Path(td), quiet=True)
            written = sorted(p for p in (Path(td) / label).iterdir())
            assert len(written) == len(ents), f"{label}: wrote {len(written)} of {len(ents)}"
            if label in count_only:
                continue
            src_data = isf_data if label == "ISF" else (SRC / label).read_bytes()
            for p, (name, off, size) in zip(written, ents):
                got = p.read_bytes()
                assert got == src_data[off:off + size], f"{label}/{p.name}: payload mismatch"
    # E19: no script calls the AVI player with a real file name (the movie-path verdict).
    from walk_isf import avip_scan
    assert avip_scan(quiet=True) == (16, 0), avip_scan(quiet=True)
    print("selftest: PASS")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="*", default=["GGD", "ISF", "SE", "VOICE", "WMSC"])
    ap.add_argument("--out", default=str(ROOT / "extracted"))
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    if a.selftest:
        selftest()
        return
    for f in a.files or ["GGD", "ISF", "SE", "VOICE", "WMSC"]:
        extract(SRC / f, Path(a.out))


if __name__ == "__main__":
    main()
