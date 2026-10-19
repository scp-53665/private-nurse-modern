"""Summarise a diagnostic run directory: blt groups, frame hashes, dumped BMPs.

Usage: python tools/frame_report.py work/tmp/run/<run>

Reads `frames.csv` (one row per qualified blit) and the `frames/` BMPs written by the
instrumented DLL, prints the groups that matter for the L2 decisions (which DC receives the
640x480 present, how many distinct frames, present-read cost), and classifies every
dumped frame as black / uniform / content so "engine renders" is a measurement and
not an opinion.
"""
import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path

HEAD = ("t_ms,label,src_hdc,src_bmp,bpp,bmp_w,bmp_h,dest_x,dest_y,dest_w,dest_h,"
        "src_x,src_y,src_w,src_h,rop,dest_hdc,dest_win,dest_class,hash,dumped,file,us")


def rows(run):
    p = Path(run) / "frames.csv"
    with p.open(newline="", encoding="utf-8") as f:
        text = [ln for ln in f if not ln.startswith("t_ms")]
    r = list(csv.DictReader(text, fieldnames=HEAD.split(",")))
    for d in r:
        d["t_ms"] = int(d["t_ms"])
        d["dest_w"] = int(d["dest_w"] or 0)
        d["dest_h"] = int(d["dest_h"] or 0)
        d["us"] = int(d["us"] or 0)
    return r


def groups(r):
    g = defaultdict(lambda: {"n": 0, "first": None, "last": None, "hashes": set(), "dumps": 0, "us": []})
    for d in r:
        key = (d["label"], "win" if d["dest_win"] not in ("0", "0x0") else "mem",
               f'{d["dest_w"]}x{d["dest_h"]}')
        e = g[key]
        e["n"] += 1
        e["first"] = d["t_ms"] if e["first"] is None else e["first"]
        e["last"] = d["t_ms"]
        e["hashes"].add(d["hash"])
        e["dumps"] += 1 if d["dumped"] == "1" else 0
        e["us"].append(d["us"])
    return g


def classify(run):
    from PIL import Image
    out = []
    for f in sorted((Path(run) / "frames").glob("*.bmp")):
        im = Image.open(f).convert("RGB")
        px = list(im.getdata())
        nonblack = sum(1 for p in px if p != (0, 0, 0)) / len(px)
        distinct = len(set(px))
        mean = tuple(round(c) for c in
                     (sum(p[i] for p in px) / len(px) for i in range(3)))
        out.append((f.name, im.size, distinct, round(nonblack, 4), mean))
    return out


def presenter(run):
    """Phase B evidence from `pn_modern.log`: the plan, the window policy, the present
    mapping and the counters (the blit CSV no longer sees the present path — those
    blits are redirected into the sink and never reach the engine's own BitBlt)."""
    import re
    p = Path(run) / "pn_modern.log"
    if not p.exists():
        return
    counts = defaultdict(list)
    first_present = last_present = None
    policy = []
    refused = suppressed = 0
    pat = re.compile(r"^\[(\d+)\] (\S+) (.*)$")
    for ln in p.open(encoding="utf-8", errors="replace"):
        m = pat.match(ln.rstrip("\n"))
        if not m:
            continue
        t, tag, rest = int(m.group(1)), m.group(2), m.group(3)
        if tag == "plan":
            print(f"plan: {rest}")
        elif tag == "window":
            policy.append((t, rest))
        elif tag == "present":
            if rest.startswith("clear"):
                continue
            first_present = first_present or (t, rest)
            last_present = (t, rest)
        elif tag == "showwindow" and "refused" in rest:
            refused += 1
        elif tag == "changedisplay" and "SUPPRESSED" in rest:
            suppressed += 1
        elif tag == "count":
            name = rest.split(" ", 1)[0]
            total = rest.split("total=")[1].split(" ")[0]
            if name.startswith("present") or name in ("GetClientRect", "ScreenToClient", "ClientToScreen"):
                counts[name].append((t, int(total)))
    print(f"\nwindow policy ({len(policy)}):")
    for t, rest in policy[:8]:
        print(f"  [{t}] {rest}")
    if first_present:
        print(f"first present [{first_present[0]}] {first_present[1]}")
    if last_present:
        print(f"last  present [{last_present[0]}] {last_present[1]}")
    print(f"mode switch suppressed={suppressed} self-minimise refused={refused}")
    for name, series in counts.items():
        if series:
            print(f"counter {name}: " + " ".join(f"{t}:{v}" for t, v in series[-6:]))


def main(argv):
    run = argv[0]
    r = rows(run)
    print(f"rows={len(r)} run={run}")
    g = groups(r)
    print(f"{'label':4} {'dest':4} {'size':10} {'n':>6} {'first_ms':>9} {'last_ms':>9} "
          f"{'uniq_hash':>9} {'dumped':>6} {'p50_us':>7} {'p99_us':>7}")
    for key, e in sorted(g.items(), key=lambda kv: -kv[1]["n"]):
        us = sorted(e["us"])
        p50 = statistics.median(us) if us else 0
        p99 = us[min(len(us) - 1, int(len(us) * 0.99))] if us else 0
        print(f"{key[0]:4} {key[1]:4} {key[2]:10} {e['n']:>6} {e['first']:>9} {e['last']:>9} "
              f"{len(e['hashes']):>9} {e['dumps']:>6} {p50:>7.0f} {p99:>7.0f}")
    win = [d for d in r if d["dest_win"] not in ("0", "0x0")]
    if win:
        print("\nwindow-DC frames by 5s bucket (hash changes = engine redraws):")
        bucket = defaultdict(set)
        for d in win:
            bucket[d["t_ms"] // 5000 * 5].add(d["hash"])
        for t, hs in sorted(bucket.items()):
            print(f"  t={t:>4}s uniq={len(hs):>3} first={min(hs)[:12]}")
    frames = classify(run) if (Path(run) / "frames").is_dir() else []
    print(f"\ndumped frames: {len(frames)}")
    for name, size, distinct, nonblack, mean in frames:
        kind = "black" if nonblack < 0.0001 else ("uniform" if distinct == 1 else "content")
        print(f"  {name:44} {size[0]:>4}x{size[1]:<4} colors={distinct:>6} "
              f"nonblack={nonblack:<7} mean={mean} {kind}")
    print()
    presenter(run)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
