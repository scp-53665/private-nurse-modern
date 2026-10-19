#!/usr/bin/env bash
# Regenerate every file in docs/evidence/ from the pristine files in original/.
# Run from the repository root:   bash tools/regen_evidence.sh
# Requires: python 3 with pefile, and `strings` (msys2/coreutils) on PATH.
set -euo pipefail
cd "$(dirname "$0")/.."
export PYTHONIOENCODING=utf-8   # console codepage on Windows is not UTF-8
EV=docs/evidence
mkdir -p "$EV"

python - <<'PY' > "$EV/E00-sha256.txt"
import hashlib, os
for f in sorted(os.listdir("original")):
    p = os.path.join("original", f)
    if os.path.isfile(p):
        h = hashlib.sha256(open(p, "rb").read()).hexdigest()
        print(f"{h}  {os.path.getsize(p):>10}  {f}")
PY

python tools/pe_report.py original/NURSEUS.EXE original/fakecdda.dll original/fakemm.dll \
    original/d3d8.dll original/d3d9.dll original/d3d11.dll original/d3d10core.dll original/dxgi.dll \
    > "$EV/E01-pe-report.txt"

python - <<'PY' > "$EV/E02-pe-imports-resources.txt"
import pefile
pe = pefile.PE("original/NURSEUS.EXE", fast_load=True); pe.parse_data_directories()
print("--- imports, full name lists")
for d in pe.DIRECTORY_ENTRY_IMPORT:
    names = [i.name.decode() if i.name else f"ord#{i.ordinal}" for i in d.imports]
    print(f"{d.dll.decode()} ({len(names)}):")
    print("   " + " ".join(sorted(names)))
print("--- resources (type -> ids)")
for t in pe.DIRECTORY_ENTRY_RESOURCE.entries:
    tid = pefile.RESOURCE_TYPE.get(t.struct.Id, t.struct.Id)
    ids = [e.name if e.name else e.struct.Id for e in t.directory.entries]
    print(f"{tid:<16} {len(ids)} {ids[:8]}")
print("--- RT_MANIFEST")
found = False
for t in pe.DIRECTORY_ENTRY_RESOURCE.entries:
    if t.struct.Id == 24:
        found = True
        for e in t.directory.entries:
            for e2 in e.directory.entries:
                print(pe.get_data(e2.data.struct.OffsetToData, e2.data.struct.Size).decode("utf-8", "replace"))
print("(none)" if not found else "")
print("--- DllCharacteristics 0x%04x" % pe.OPTIONAL_HEADER.DllCharacteristics)
print("--- debug directory entries:", len(getattr(pe, "DIRECTORY_ENTRY_DEBUG", [])))
PY

python tools/find_english.py original/NURSEUS.EXE original/ISF original/GGD original/SE original/VOICE \
    original/WMSC original/NURSEUS.SAV original/NURSEUS.SUF original/NURSEUS.HLP \
    > "$EV/E03-plaintext-scan.txt"

python tools/probe_text_encoding.py original/ISF > "$EV/E04-text-encoding-frequency.txt"
python tools/decode_isf_text.py > "$EV/E05-dialogue-decode.txt"
python tools/probe_ggd_images.py > "$EV/E06-ggd-image-probe.txt"
python tools/probe_gad_header.py >> "$EV/E06-ggd-image-probe.txt"
python tools/probe_ggd_bpp.py > "$EV/E07-ggd-bpp-probe.txt"
python tools/unpack.py --selftest > "$EV/E08-archive-selftest.txt"
python tools/decode_ggd.py --selftest > "$EV/E17-ggd-decode.txt"
python tools/probe_isf_opcodes.py > "$EV/E18-isf-opcodes.txt"

python - <<'PY' > "$EV/E09-exe-select-strings.txt"
import re
d = open("original/NURSEUS.EXE", "rb").read()
pats = [rb"Software\\[^\x00]{2,40}", rb"[A-Za-z]:\\\\[^\x00]{3,60}", rb"\.GGD", rb"\.GAD", rb"\.ISF",
        rb"\.SAV", rb"MUON\.WAV", rb"\.MP3", rb"\.WAV", rb"\.TXT", rb"http[^\x00]{4,60}"]
for p in pats:
    hits = sorted(set(re.findall(p, d)))
    print(p.decode("latin1"), "->", [h.decode("latin1") for h in hits][:14])
PY

strings -n 3 original/NURSEUS.EXE | grep -xE "GGD|ISF|SE|VOICE|WMSC|NURSEUS\.SAV|NURSEUS\.SUF|MUON\.WAV|\.GAD" \
    | sort -u > "$EV/E10-exe-nul-terminated-strings.txt"

# E10 only sees strings of >= 3 chars, so the 2-character archive name "SE" cannot
# appear there; E15/E16 use explicit patterns instead.
strings -n 3 original/NURSEUS.EXE \
    | grep -E 'MS Sans Serif|^System$|^CFont$|CGDTPropertySheetSystem|AU_AFX|^CreateFont|FileTimeToSystemTime|^GetSystem|^SystemParametersInfoA|SETFONT' \
    | sort -u > "$EV/E15-exe-font-strings.txt"

strings -n 3 original/NURSEUS.EXE | grep -xE '[A-Za-z0-9_]+\.[dD][lL][lL]' \
    | sort -u > "$EV/E16-exe-dll-name-strings.txt"

python - <<'PY' > "$EV/E11-isf-build-artifacts.txt"
import struct
d = open("original/ISF", "rb").read()
count, = struct.unpack_from("<I", d, 8)
for i in range(count):
    off = 0x20 + i * 20
    n = d[off:off + 12].rstrip(b"\0").decode("latin1")
    o, s = struct.unpack_from("<II", d, off + 12)
    if n.lower().endswith((".log", ".txt")):
        blob = d[o:o + s]
        txt = blob.decode("cp932", "replace")
        lines = [l for l in txt.splitlines() if l.strip()]
        print(f"=== ISF entry {n!r} offset=0x{o:x} size={s}")
        print(f"    decoded cp932: {len(txt)} chars, {len(lines)} non-empty lines, "
              f"{txt.count('.txt')} source-file references")
        # The body is a developer build log. Only its shape is evidence; the text
        # itself is not copied into the repository.
PY

python tools/probe_isf_bytecode.py > "$EV/E12-isf-structure.txt" 2>&1
python tools/walk_isf.py --scan >> "$EV/E12-isf-structure.txt" 2>&1

# E13: corpus size as a statistic. E14: a synthetic decode example. The real text
# is the game's own and is never written into the repository.
python tools/decode_isf_text.py | head -1 > "$EV/E13-dialogue-extract.txt"
python - <<'PY' > "$EV/E14-dialogue-sample.txt"
import sys
sys.path.insert(0, "tools")
import decode_isf_text as dec

msg = 'Hello, world! "Quote" 42.'
codes = [0xFF] + [4 * (ord(c) & 0x3F) | (1 if c.isalpha() else 0) for c in msg]
print("### synthetic sample, not game text")
print(f"text   : {msg}")
print(f"codes  : {' '.join(f'{c:02X}' for c in codes)}")
print(f"decoded: {''.join(dec.char(c) for c in codes)}")
PY
python tools/decode_isf_text.py >> "$EV/E14-dialogue-sample.txt"

# E19: the movie path (s13). AVIP's operand format and file naming in the EXE, every
# AVIP-shaped byte run in the scripts, and the stock CD-audio shim's MCI strings (the
# "ActiveMovie Window" seen at runtime is MCI mpegvideo playing TKxx.MP3, not a movie).
{
    echo "--- NURSEUS.EXE: AVIP strings (opens VideoDir + name + .AVI)"
    strings -n 4 original/NURSEUS.EXE | grep -E '^(VideoDir|\.AVI|%s%s)$' | sort -u
    echo "--- ISF: AVIP-shaped byte runs"
    python tools/walk_isf.py --avip
    echo "--- fakecdda.dll (stock CDDA->MP3 shim): MCI command strings"
    strings -n 5 original/fakecdda.dll | grep -E 'MP3|mpegvideo|^play|^open'
} > "$EV/E19-movie-path.txt" 2>&1

echo "evidence regenerated in $EV"
