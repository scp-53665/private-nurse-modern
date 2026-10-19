"""Observed structure of ISF entries: sizes, text-run layout, raw dump of one script.

Nothing here claims to understand the bytecode; it records what is measurable so a
reviewer can check the claims in docs/02-formats.md.
"""
import collections
import struct
import sys
from pathlib import Path

import decode_isf_text as dec

PN = Path(__file__).resolve().parent.parent / "original"


def main():
    ents = list(dec.entries(PN / "ISF"))
    print(f"ISF entries: {len(ents)}")
    buckets = collections.Counter()
    for _n, b in ents:
        buckets[min(len(b) // 1024, 64)] += 1
    print("size bucket (KiB, capped at 64):", sorted(buckets.items()))
    runs_total = 0
    chars_total = 0
    no_text = 0
    for _n, b in ents:
        runs = dec.text_runs(b)
        if not runs:
            no_text += 1
        runs_total += len(runs)
        chars_total += sum(len(c) for _o, c in runs)
    print(f"text runs (message lines): {runs_total}   decoded chars: {chars_total}   entries with no text: {no_text}")

    name, blob = next((n, b) for n, b in ents if n == "A0_0.ISF")
    runs = {off: codes for off, codes in dec.text_runs(blob)}
    print(f"\n--- {name} ({len(blob)} bytes)")
    print("text-run map, one line per 16-byte row, 'T' where a decoded run starts:")
    covered = 0
    for i in range(0, len(blob), 16):
        mark = "T" if any(i <= k < i + len(codes) * 2 for k, codes in runs.items()) else " "
        if mark == "T":
            covered += 1
        print(f"{mark} {i:04x}")
    print(f"rows with a run start: {covered}   runs: {len(runs)}")
    print("\nfirst 4 message offsets and lengths:")
    for off, codes in dec.text_runs(blob)[:4]:
        print(f"  @0x{off:x} ({len(codes)} chars)")
    if "--dump" in sys.argv:
        # Local only: the payload is the game's own file and must not be committed.
        print("\n--- raw hex dump (local only)")
        for i in range(0, len(blob), 16):
            chunk = blob[i:i + 16]
            ascii_ = "".join(chr(c) if 32 <= c < 127 else "." for c in chunk)
            print(f"  {i:04x} {chunk.hex(' '):<48} {ascii_}")


if __name__ == "__main__":
    main()
