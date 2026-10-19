# Private Nurse (AngelSmile 2002 / GamelexGS-JAST US release) — reverse-engineering knowledge base

Research notes for the modernisation patch. You do not need this folder to install, use or
build the patch. Start at the [project README](../README.md) for that.

Scouting + planning artefacts for modernising a fixed-resolution, 2002-era Japanese VN engine
that renders through **Win32 GDI**, plays audio through **DirectSound + MME (WINMM) + MSACM**,
and stores everything in five custom archive files.

The repository contains no game data and no game text. `original/` is the pristine
user-supplied game directory. It is git-ignored, must never be modified, and is hash-pinned
by `E00`.

## Read in this order

| Doc | What it answers |
|---|---|
| [`00-scouting-report.md`](00-scouting-report.md) | What the engine is; which assumptions in the brief are true/false; what is still unknown |
| [`01-stack.md`](01-stack.md) | Component-by-component stack and its status on Windows 11 |
| [`02-formats.md`](02-formats.md) | File formats, verified parsers, the solved dialogue text encoding |
| [`03-plan.md`](03-plan.md) | Modernisation options, cost/risk, recommendation, open decisions |
| [`../AGENTS.md`](../AGENTS.md) | Working rules for agent sessions on this repo |

## Evidence and citation protocol

Research provenance: the docs were written during working sessions. They cite two kinds of
local file that are not part of this repository. `work/handoff/*.md` are session journals
and `work/tmp/*` are run logs and screenshots. Both were kept on the author's machine when
the repository was published. Treat those citations as unavailable and rely on the evidence
ids below, which are all reproducible.

Every factual claim in these docs carries an **evidence id** `E00`–`E19`. Each id is one file in
[`docs/evidence/`](evidence). All of them are regenerated from `original/` by one command:

```
bash tools/regen_evidence.sh        # run from the repo root; PYTHONIOENCODING is set inside
```

`E00-sha256.txt` pins the exact inputs, so a reviewer can confirm they are looking at the same
bytes this session did. Cite as `E02 §USER32` or `E01 §"===== original/NURSEUS.EXE"`, and give the
`grep -n` anchor rather than a bare line number, because line numbers shift if a tool changes.

Claim tags used throughout:

* **OBSERVED** — a tool output, byte range or hash shows it directly.
* **INFERRED** — explained by observations, but not directly shown (state why).
* **UNKNOWN** — no evidence yet; the doc says what would settle it.

Anything tagged UNKNOWN was deliberately *not* guessed at.

## Tooling in `tools/`

| Tool | Purpose | Verified by |
|---|---|---|
| `unpack.py` | Extract GGD/ISF/SE/VOICE/WMSC; `--selftest` asserts every format invariant | `bash tools/regen_evidence.sh` → `E08` |
| `decode_ggd.py` | Decode GGD/GAD pixels (`--png` renders); `--selftest` decodes all 1036 | `E17` |
| `decode_isf_text.py` | Decode ISF dialogue (`--dump FILE` writes the whole corpus) | `E05`, `E14` |
| `pe_report.py` | PE headers, sections, imports/exports, version info | `E01` |
| `probe_text_encoding.py` | Derivation of the character encoding (frequency analysis) | `E04` |
| `probe_isf_opcodes.py` | ISF opcode table + VM handler map (117 ops) | `E18` |
| `probe_isf_bytecode.py` | ISF entry sizes, text-run layout, run-start map | `E12` |
| `walk_isf.py` | ISF bytecode tiling (`--scan`), one scene end to end (`--scene`), every AVIP-shaped byte run (`--avip`, the movie-path verdict) | `E12`, `E19` |
| `probe_ggd_images.py`, `probe_ggd_bpp.py`, `probe_gad_header.py` | GGD/GAD inner container, tag offsets, bits-per-pixel bounds, GAD frame records | `E06`, `E07` |
| `find_english.py` | Counts sentence-shaped runs in each shipped file, to show the dialogue is encoded rather than plaintext | `E03` |
| `regen_evidence.sh` | Regenerates **all** of `docs/evidence/` (E00–E19) | — |

Requires Python 3 (3.13 used here) with `pefile`, plus `strings` from msys2 on `PATH`.
`dumpbin` and `7z` are **not** installed on this machine.

### Known residuals (not bugs, tracked)

* **716/20 115 bytecode gaps do not tile statically** (3.6%: 582 operand
  overruns, 107 ONJS-table overruns, 19 path-dependent ops, 8 singletons).
  Verified ops whose operands would extend into genuine text, or whose count
  is runtime-selected. Static tiling stops rather than consume text; resolving
  them needs a runtime fetch trace. Open with U2.
* **`ANGEL3.GAD` does not match the other `.GAD` sheets** (tag at 86, not 278). The `GAD `
  header fields are UNKNOWN; U6 starts there.

## Environment traps (hit this session)

* Console codepage is **cp932**, so `print()` of non-ASCII raises `UnicodeEncodeError`.
  Always `export PYTHONIOENCODING=utf-8` (the regen script does).
* `unpack.py` writes 343 MB (`GGD`) and 136 MB (`VOICE`) when extracting — point `--out` at a
  volume with room, or pass a single archive.
* Path handling is split: `pe_report.py` and `find_english.py` resolve their **argv relative to
  the cwd**; every other tool resolves `original/` relative to its own `__file__` and runs from
  anywhere. Only `bash tools/regen_evidence.sh` must be invoked by a path reaching the repo.

## Glossary

`SM2MPX10` — container magic shared by `GGD` and `ISF`.
`GGD`/`GAD` — image containers (1036 entries: backgrounds, character sprites, animation sheets).
`ISF` — compiled scenario container (803 entries: per-scene bytecode + the dialogue text).
`SE` — 77 sound-effect WAVs. `VOICE` — 2715 voice WAVs. `WMSC` — 1 WAV (`MUON.WAV`, ADPCM BGM).
`SUF` — startup-info INI (`Company`/`Key`/`Title`/`HelpName`). `SAV` — 64,246-byte binary save.
`fakecdda.dll`/`fakemm.dll` — 2009 shim pair that turns CD-audio calls into MP3 playback; the EXE
imports from these **statically**.
