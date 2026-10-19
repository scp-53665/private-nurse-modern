"""Hunt for English dialogue: long runs that look like sentences (spaces, letters, punctuation).

Counts only by default, because the matched runs are the game's own text and must
never be committed. Pass --samples to print the longest runs locally.
"""
import re
import sys
from pathlib import Path

PN = Path(__file__).resolve().parent.parent / "original"
SENT = re.compile(rb"[A-Za-z][A-Za-z0-9 ,'\.\!\?\-]{20,}")

samples = "--samples" in sys.argv
for name in [a for a in sys.argv[1:] if not a.startswith("--")]:
    p = Path(name)
    d = p.read_bytes()
    runs = SENT.findall(d)
    words = sum(len(r.split()) for r in runs)
    print(f"{name}: sentences>=21chars: {len(runs)} totalWords~{words}")
    if samples:
        for r in sorted(runs, key=len, reverse=True)[:8]:
            print("   ", len(r), r[:100].decode("latin1"))
