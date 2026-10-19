# 02 — File formats

All layouts below are implemented in `tools/unpack.py` and **asserted** by
`python tools/unpack.py --selftest` (evidence `E08`, last run `selftest: PASS`). Regenerate with
`bash tools/regen_evidence.sh`. Tags: OBSERVED / INFERRED / UNKNOWN.

## 1. SM2MPX10 — `GGD` and `ISF`

```
0x00  u8[8]   "SM2MPX10"
0x08  u32     entryCount
0x0C  u32     dataStart   (== 0x20 + entryCount*20, asserted)
0x10  char[12] rootName   ("GGD" / "ISF", NUL-padded)
0x1C  u32     tableOffset (0x20, asserted)
0x20  entryCount * { char[12] name; u32 offset; u32 size }   (20 bytes)
      zero padding up to align16(dataStart)   (asserted; GGD 0 B, ISF 4 B)
      payloads, each start aligned up to 16 bytes (gap after payload == 0 for every
      entry, asserted), last payload ends within 16 bytes of EOF (asserted;
      measured tail is 11 B for GGD, 15 B for ISF)
```

| Archive | Entries | Payload bytes | First → last entry | Evidence |
|---|---|---|---|---|
| `GGD` | 1036 | 343 631 962 | `AFTER.GGD` → `YU_007.GGD` | E08 |
| `ISF` | 803 | 2 618 990 | `-C-O\x99\xC5.TXT` → `TU_END3.ISF` | E08 |

The **first payload offset is not asserted by the file**; it is `align16(dataStart)` in
both archives (`GGD` 0x5110 with no pad, `ISF` 0x3edc → 0x3ee0 with 4 zero bytes).
This is inferred from the two samples and asserted in `tools/unpack.py`; it was found by
mutation testing, not stated anywhere in the original engine's own metadata.

Names are read as 12 NUL-padded bytes (not NUL-terminated), so entries can carry non-ASCII bytes
(`-C-O\x99\xC5.TXT`; the two bytes `99 C5` decode as one Shift-JIS kanji, so this entry is a
short Japanese development note, not the four-kanji name once guessed here).

## 2. `SE`, `VOICE`, `WMSC` — audio archives

```
0x00  u16   tableSize           (multiple of the record size, asserted)
0x02  tableSize/recSize * { char[12] name; u32 absoluteStart; ... }
      payloads follow the table, sequential, no padding
```

* `SE`, `VOICE`: 16-byte records. `WMSC`: 32-byte records
  (`char[12] name; u32 absoluteStart; u8[12] zero; u32 endOffset`, the value at record
  offset 12 in both cases). `WMSC`'s `endOffset` equals the file size — it is the only
  EOF pin WMSC has, since its single record is named rather than empty.
* **The `u32` is an absolute file offset, not a size** (OBSERVED, corrected in this session): the
  first record's value equals `2 + tableSize` (the table end, i.e. where the first payload starts),
  offsets increase strictly monotonically, and the last record has an **empty name whose value
  equals the file size** (EOF marker). A file's size = next record's `absoluteStart` − its own.
  Proof it is not a size: summing all 78 `SE` record values gives `0x10B7EDCD`
  (280 489 421 B), which would place the end of the archive at `0x4E2 + 0x10B7EDCD`
  against an actual file size of `0x976E94` (9 924 244 B) — an overrun of ~270 MB.
  Note that `sum(sizes) == filesize − table_end` also holds (9 922 994 / 136 126 808 /
  352 940 for SE/VOICE/WMSC) but proves nothing about interior offsets: shifting an
  interior boundary only moves bytes between two adjacent files, leaving the total intact.
* `SE`: tableSize 1248 → 78 records → **77 files** (`AY_0412.WAV` … `TA_0055.WAV`, 9 922 994 B).
* `VOICE`: tableSize 43456 → 2716 records → **2715 files** (`AO_0001.WAV` … `YU_0035.WAV`,
  136 126 808 B).
* `WMSC`: tableSize 32 → 1 record → **1 file** `MUON.WAV` (352 940 B of payload). No EOF marker here;
  the size is `filesize − 0x22`. The record's bytes 16..27 are twelve zero bytes (asserted).
* **Every payload in all three audio archives begins `RIFF….WAVE` and is ≥ 44 B** (asserted).
  This is the pin that actually holds the interior offsets; see the note above on why the
  size-sum does not.

**INFERRED / not in this evidence set:** `MUON.WAV` is MS-ADPCM 2-channel 22050 Hz, and the `SE`/
`VOICE` WAVs are ADPCM too (consistent with an EXE message about missing ADPCM support, counted
in E03). Re-verify by dumping the WAV
headers; a `tools/wavinfo.py` does not exist yet.

## 3. `GGD` / `GAD` payload — image container (partially reversed)

There are two inner-container layouts, and **the tag offset is per-entry, not constant**:

```
GGD entry:  payload+0x000 : u8[4] tag B9 AA B3 B3
            payload+0x004 : u16 width
            payload+0x006 : u16 height
            payload+0x008 : pixel data (codec unknown)          -- 1032 entries

GAD entry:  payload+0x000 : u8[4] magic "GAD " (ASCII, 0x47414420)
            ...            : header, length varies per entry
            payload+N      : u8[4] tag B9 AA B3 B3 + u16 w + u16 h, then pixels
```

`N` is **278 (0x116)** for `ANGEL.GAD`, `ANIME.GAD`, `CDF08.GAD`, and **86 (0x56)** for
`ANGEL3.GAD`. Tag-offset histogram over all 1036 entries: `{0: 1032, 86: 1, 278: 3}`
(E06). The offset is a **field, not a constant**: `u32` at payload+4 holds the absolute tag
offset, `u32` at +8 is the image count (always 1 — consistent with exactly one tag per
entry). The 4 `u16` at +12 are a per-sheet prologue of unknown meaning
(`20 8c be 168` for ANGEL/ANIME, `08 8c be 168` for ANGEL3, `20 f0 c8 a0` for CDF08).

Then come **frame records**: `N` × 4 `u16`, followed by one trailing zero word (E06,
`tools/probe_gad_header.py`, all asserts passing). Record 0 of each BASE-group is
`(S,0,0,150)`; the rest are `(0,BASE,k·S,150)` for k = 1..7 (ANGEL/ANIME/ANGEL3) or k = 1..31
(CDF08), with step S = `0x5a` (90) except CDF08 (`0x28` = 40). BASE is a per-group word:
`0x0/0x168/0x2d0/0x438` in ANGEL (= 0/360/720/1080 — suspiciously 360·n), `0x0/0x169/0x2d1/0x0`
in ANIME, always `0x0` in CDF08/ANGEL3. Counts: ANGEL 32, ANIME 32, CDF08 32, ANGEL3 8 —
and **ANGEL3's 8 records are byte-identical to ANGEL's first group**. ANIME differs from ANGEL
in exactly the 24 records covered by the non-zero BASE groups (BASE+1, last group zeroed).
The `(x,y,w,h)`-as-rect reading fits every record except record 0 (w = 0), so the record
semantics — rect list, blit script, delay table (BASE as milliseconds?), flags — are UNKNOWN.
What the `GAD ` header fields after the tag offset mean is likewise UNKNOWN beyond the parse.

| Entry | Payload size | Tag offset | w×h | Evidence |
|---|---|---|---|---|
| `AFTER.GGD` | 892 774 | 0 | 640×480 | E06 |
| `AFTER_P.GGD` | 96 097 | 0 | 176×200 | E06 |
| `AFTERA.GGD` | 835 443 | 0 | 640×480 | E06 |
| `ANGEL.GAD` | 852 703 | 278 (0x116) | 1440×720 | E06 |
| `ANIME.GAD` | 852 703 | 278 | 1440×720 | E06 |
| `ANGEL3.GAD` | 235 325 | **86 (0x56)** | 360×720 | E06 |
| `CDF08.GAD` | 164 907 | 278 | 160×1280 | E06 |

All 1036 `GGD` entries carry **exactly one** tag — E06 now prints the full tag-count
histogram `{1: 1036}`, which previously was only claimed, not measured.

**Bits per pixel — solved (U1 closed).** Op stream at payload+8 (after tag+w+h),
decoded by `tools/decode_ggd.py` (`--selftest` decodes all 1036 entries, E17).
Bytecodes (OBSERVED — recovered from the EXE's own op dispatch at `0x40827d`,
jump table `0x408438`, cases `0x408292`–`0x4083e0`):

| op | operands | meaning |
|---|---|---|
| `0x00` | u8 N | N copies of the previous triplet (N=0: no-op) |
| `0x01` | u8 L, u8 O | copy L triplets from O triplets back |
| `0x02` | u8 L, u16LE O | copy L triplets from O triplets back |
| `0x03` | u8 O | copy 1 triplet from O triplets back |
| `0x04` | u16LE O | copy 1 triplet from O triplets back |
| `0x05`–`0xFF` | (op−4) RGB triplets | literal pixels |

Copies advance byte-wise (overlap = forward memmove); out-of-range reads yield
zero (fresh-buffer semantics). Decode stops at w*h pixels; trailing bytes, if
any, are ignored. Channel order is RGB as stored; rows are top-down.
Verified by rendering: `AFTER.GGD` (title screen, legible "After Diary" menu),
`AFTER_P.GGD` sprite, `ANGEL.GAD` 1440×720 32-frame sheet and `ANGEL3.GAD`
360×720 8-frame sheet (all pixel-exact, viewed). `MOJI_*.GGD` decode to
near-empty cells (≈360 non-black px: white core + 1-bit fringe) — sparse
content, not a decode failure. GAD sheets decode from their tag offset
(278 / 86) with the same codec.
* **UNKNOWN (reduced):** the `GAD ` header fields and frame-record semantics
  (offsets/counts parsed, meaning open — U6); which op the encoder prefers
  needs no further work for decoding.

## 4. `ISF` — compiled scenario

803 entries: **801 `*.ISF`** scenario objects + two build artifacts:

* `-C-O\x99\xC5.TXT` (1 095 B, offset 0x3EE0): a Shift-JIS developer note about half-width
  character handling. It lists four script command mnemonics and states that after this change
  a string may be delimited by either single or double quotes (E11).
* `ERROR.LOG` (100 102 B): **801 absolute source paths**, each followed by a conversion
  status line. The paths name a build share and a `<scene>.txt` file (E11, E03). The
  developer text is not reproduced here.

**INFERRED:** the English release's dialogue was compiled from those `.txt` files by a converter
that ships with the game — the EXE contains a compile-failure message (E03), and the
developer note and the log messages are in the converter's own Japanese.
The 801 names are also a free map of the scene structure (`day10_21.txt` style).

Entry sizes (E12): median ≈ 2 509 B, min 14 B, max **45 023 B among the 801 `*.ISF`**
(`GAME.ISF`); the largest entry overall is `ERROR.LOG` at 100 102 B. Size histogram in KiB
buckets from E12; 74 entries contain no text (74 = system/menu scripts).

### 4.1 Dialogue text — SOLVED

A message is a run of `<code> 0xFD` **byte pairs**. This is deliberately *not* described as a
16-bit word stream: runs begin at arbitrary byte offsets. `0xFD` bytes sit at even entry-relative
offsets 359 853 times and at odd offsets 358 207 times (E05); the even-aligned `u16` view of E04
sees only the odd-offset half (49.9 %). Run markers split 10 028 even vs 10 086 odd (E05). A run
always **starts** with the pair `FF FD` (word `0xFDFF`) and the value `0xFF` never appears inside a run.

```
byte 0   byte 1      meaning
0xFF     0xFD        0xFDFF: start of message (20115 occurrences = one per run, E05)
<code>   0xFD        one character; code = 4*(ascii & 0x3F) | class
```

| `code` | decoding | example |
|---|---|---|
| `class = code & 3 == 1` | `x = code >> 2`; `x` in 1..26 → `'A'..'Z'`; `x` in 33..58 → `'a'..'z'` | `0x85` → `a`, `0x95` → `e`, `0x05` → `A` |
| `class = code & 3 == 0` | `chr(code >> 2)` when `0x20 ≤ code>>2 < 0x40` | `0x80` → space, `0xB0` → `,`, `0x9C` → `'`, `0xB8` → `.`, `0x88` → `"`, `0xFC` → `?` |
| `class = code & 3 == 2 or 3` | **never occurs as a character** (only `0xFF` ≡ 3, the run marker) | — |

The two classes exist precisely so that `'!'` (class 0, `x`=33 → `0x84`) and `'a'` (class 1, `x`=33
→ `0x85`) do not collide — the low 6 bits are shared, the class bit disambiguates.

**Evidence / derivation** (all executed, `tools/probe_text_encoding.py` + `tools/decode_isf_text.py`):

* E04: across all `ISF` entries, the even-aligned `u16` view carries **358 207** words with high
  byte `0xFD` spanning **87** distinct codes; the frequency order is `0x80` 57 217 (space, 16 %),
  `0x95` 31 051 (`e`), `0xD1` 22 293 (`t`), `0xBD` 21 889 (`o`), `0x85` 20 979 (`a`),
  `0xCD` 17 329 (`s`), `0xB9` 16 203 (`n`), `0xA5` 15 467 (`i`), `0xA1` 14 990 (`h`),
  `0xC9` 14 197 (`r`) — i.e. English letter frequencies. That produced the hypothesis
  `code = 4*(ascii & 0x3F) | class`. Note **87** is E04's even-aligned code count; over the
  *whole* corpus there are **78** distinct character codes (E05).
* E05: applying it decodes **737 989 characters in 20 115 message runs across all 803 entries with
  zero unmapped codes** (`0xFF` is the only "unmapped" value and is exactly the run marker).
  Four messages at known offsets are pinned by offset and length in `unpack.py --selftest`.
  The decoded text is the game's own and is not reproduced here.
* E13: the corpus size as a statistic. `tools/decode_isf_text.py --dump <file>` writes the
  full corpus locally; the file is never committed.

**The lone terminator.** Every admitted run (20 115/20 115, asserted per-run in
`tools/unpack.py --selftest`) is followed by `[valid-code][0x00]`: the message's final
character with a `0x00` second byte where every other character has `0xFD` (equivalently
an unpaired final code plus NUL — either way the next bytecode starts after the `00`).
`text_runs()` appends it to the run. Every non-header gap *follows* a lone terminator —
i.e. opens with the next bytecode, almost always `ED` (`0x00`): gap openers are
`00 00 08` (ED, then ONJS) ×20 068, `00 00 e9` ×27, `00 08 1c` ×10, and 8 rarer
three-byte patterns (all 20 115 non-header gaps accounted for).
Lone distribution (OBSERVED, E05/E14):
`.` ×12 122, `"` ×6 867, `?` ×523, `,` ×332, `!` ×208, plus a genuine tail (E05 top-8
covers through `)` ×3: trailing space ×32, `'` ×22, `)` ×3 inside an onomatopoeia)
— and, from a local corpus decode, `:` lead-ins in two scenes
plus three mid-word single letters in three scenes that only read correctly with the lone
appended. The 17 gaps that start with a valid code but are *not* lone terminators are all
file headers
(`gap@0x0` of `CG01`–`CG14`, `CGRATE`, `DIARY`, `RANDOM_M` — system files with
multi-KiB `u32` headers, E12); the lone lives at the *end* of the preceding text span,
never at a gap start.

**The `min_len=3` filter.** At `min_len=1` there are 20 301 pair-runs; 186 are single
pairs and stay dropped — `+` (code `0xAC`) ×137 across 129 files, `{00}` ×26 across 12
system files (`CG_ALL` 6, `CGRATE`/`RANDOM_M`/`RANDOM_A` 3 each, `CD`/`CD_TEST`/`GAME` 2
each, `MAIN`/`MUSIC`/`CG01`/`CG08`/`CG14` 1 each), `{18}` ×6, `{10}` ×5, `{14}` ×5,
`{0C}` ×2, `{04}`/`{74}`/`{17}`/`{E3}`/`{08}` ×1 — structural bytes coinciding with an
`0xFD` byte, e.g. `… 1c 0d 31 2d c8 00 | ac fd 00 | ff fd 5d fd …`. One 2-pair
run is admitted *through* the filter by its lone terminator: one two-letter word in one
scene that carries bytecode between its letters, so the pair run on its own looks like a
fragment. So "737 989 characters / 20 115 runs" is the `min_len=3` +
lone figure. What *is*
exact is the byte budget: **every `0xFD` byte in the ISF payload region is accounted for by the
pairing, with no leftovers** (`payload_region.count(0xFD) == 718 060`, asserted on the raw
pairs). The ISF entry table itself contributes 9 further `0xFD` bytes (e.g. entry #323 has
`size == 0x5FD`) — those are size fields, not text.

**Residual unknown (closed).** The old "7 030 `"` with zero closers" signature is gone: the
6 867 lone `"` are the missing closers. No third code class is needed — 78 distinct codes,
zero unmapped, quotes balanced.

`tools/unpack.py --selftest` asserts four decoded sentences spanning letters, punctuation,
apostrophes and quotes, the corpus-wide totals, the class-2/3 boundary, the unmapped-code
set, the `0xFD` budget, a per-run lone-terminator pin, and the `"` count — so a wrong formula,
a wrong class split, a dropped terminator or a punctuation regression all fail the self-test.

### 4.2 Scenario bytecode — ISA recovered, VM partially reversed (U2)

`A0_0.ISF` (2 761 B) annotated hex dump is in E12. Shape: `u32 0x0000000C` at 0x00, then a few
u32s, the first text run at **0x22**, bytecode at **0x47–0x4f**, then the next run at **0x55**.
So the file is `[header][text][bytecode][text][bytecode]…`.

**OBSERVED.** The EXE embeds the full ISA as developer documentation (`0xNN<TAB>mnemonic :
description`, 117 ops `0x00`–`0xFF`, E18): `0x00 ED` (end), `0x01 LS` / `0x02 LSBS`
(load scenario), `0x04 JP` / `0x05 JS` / `0x07 ONJP` / `0x08 ONJS` (jumps),
`0x10`–`0x19` command-window ops, `0x20`–`0x2E` message-window ops
(`0x2B PM` text display, `0x2C PMP` voice-checked display, `0x2D WSH` hide,
`0x2E WSS` show), `0x30`–`0x47` flags/variables (`0x31 SK`, `0x33 HF`, `0x44 CALC`,
`0x47 IF`), `0x50`–`0x6A` graphics (`0x56 GL` load+display, `0x65 GAOPEN` animation),
`0x70`–`0x7A` audio (`0x70 ML` music, `0x74 SER` / `0x75 SEP` SE, `0x77 PCMON` voice,
`0x90`–`0x92` CDDA), `0xF1`–`0xF2` waits, `0xFE BREAK`, `0xFF EXT`.
The runtime VM loop is at `0x424e30`: each op byte is followed by a prefix
operand (u8, or 15-bit if the high bit is set), then dispatch through the byte
map at `0x42576c` into jump table `0x42554c` (136 handlers, op→handler in E18).
Each handler stub calls one implementation function (`0x424e20`–`0x42bde0`;
per-op stub/impl addresses in E18, read from the EXE bytes by
`tools/probe_isf_opcodes.py`, which fails loudly instead of misattributing).

**OBSERVED (operand table, ~110 ops).** Every impl's stream reads were recovered
by disassembling from its exact address (`objdump -j .text --start-address`;
the linear `objdump` dump desyncs in places, e.g. around `0x428000`,
so a linear disassembly is never trusted for reader lists). Readers are six functions:
u8 `0x424970`, u16 `0x424990`, u32 `0x424980` (raw), VAL4 `0x4249a0` (tagged,
always 4 bytes), STR13 `0x4249c0` (cap 13), STR28 `0x4249f0` (cap 28). The
string readers consume up to cap bytes and stop early at NUL -- they do not
require a NUL in the stream. All encoded sizes are reads before the impl's
first branch (unconditional); the full per-op table lives in
`tools/walk_isf.py: EXTRA` with impl address per entry. Verified two ways:
ground-truth re-disassembly of every encoded impl matches its EXTRA entry
(CPS corrected 18→19 this way), and the walker tiles with them (below).
Notable shapes: `SK` = u16+u8; `SKS` = u16+u16+u8; `CW`/`WS` = u8+4×VAL4+u8;
`GSCRL` = u8+14×VAL4; `GP` = u8+16×VAL4; `EXC` = 3×VAL4+u8; `GL` = VAL4+STR28;
`E4` = 4×u8+3×STR28+u8+3×VAL4+u8+2×u16; dialog ops (`E0`/`E2`) and loaders
(`GL`, `CNF`, `AVIP`, `ML`, `SER`, `PCMON`...) take STR13/STR28 filenames;
`GF`/`MS`/`PCMEND`/`SRET`/`RT`/`68`/`73`/`7A`/`F2`/`F3...` variants take no
operands beyond op+prefix. `KIDSET` (`0xA3`) reads its u32 only when
`[ctx+0x1a8]!=0 and [ctx+0x1b0]==0`, else nothing -- the one conditional
operand found; the walker assumes the 4-byte form (1311/1312 occurrences
tile with it; the skip path is a runtime caveat, noted below).
`HN`/`HF`/`HB` share impl `0x427360` (2×u16, jump via `0x425870`, which reads
no stream). `HXP` (`0x3C`) and `ED` have no impl call at all.
Path-dependent (static size unknowable, left unverified): `GP` (u8+7×VAL4,
then a switch), `IF` (VAL4+u8+VAL4, then a condition mini-interpreter with a
loop), `IH` and `CALC` (reads after runtime branches), `HP` (branches before
all reads). 19 gaps blocked on these five.

**OBSERVED (message-tag tail).** After tiling ops, 1-2 bytes remain before the
next text marker in 12 234/19 399 tiled gaps. Those bytes are message
metadata, not code: nonzero values occur *only* before quoted (dialogue)
lines (100% for one-byte tags: 04→1063/1063, 08→569/569, 0C→162/162;
two-byte `10 XX` tails: `10 00`/`10 08`/`10 04`, all 100% quoted) and cluster per
scene (DA→04/08, MI→0x0C), while a code reading is impossible for the 1-byte
and `10 XX` cases -- `ED`(`00`) would fall through into text, `JP`(`04`)/
`ONJS`(`08`) would take jump targets and table entries from text bytes.
Two-byte `OP 00` tails (~400 cases: `12`/`22`/`2A`/`42`/`32` + `00`, i.e.
CIR/WL/WC/HINC/SKS-shaped, 0-4% quoted) are likewise consumed as TAG but
flagged AMBIGUOUS: they parse cleanly as truncated known ops and lack the
quote signal, so the rule may mask real operand overruns there. The walker
consumes 1-2B tails as `TAG` entries (8563× 1 byte, 3366× 2 bytes, 99 distinct
values) and never consumes text; overruns needing 3+ bytes stay blocked.
Zero-tags (6634×`00`, 21% quoted = baseline) are structurally identical but
semantically ambiguous (default tag vs truncated `ED`); marked UNKNOWN.

**OBSERVED (scene walk, U2 static acceptance).** `tools/walk_isf.py --scene NAME`
disassembles one scene end to end (every byte: header u32s, ops with decoded
prefix/operands, TAGs, text). 332/729 dialogue scenes tile 100% (plus 72
system files with no dialogue); the largest is `A_H11.ISF` (107 gaps, 10 827
B). Per-gap totals (E12): **19 399/20 115 tiled (96.4%), 12 234 with TAG**;
716 blocked (582 operand overruns, 107 ONJS-table overruns, 19 path-dependent,
8 single-occurrence unknown ops). VAL4 values display with their static tag
(`LIT`/`VAR`/`RND`/`SIGN` per the `0x42c450` bit rules); jump targets stay
state-dependent (scenario-pointer table `0x425870`), so control-flow edges
are annotated, not resolved -- full branch-following needs a runtime trace.

**UNKNOWN (U2 remainder, each with a precise bound).**
* ~582 gaps where a verified op's operands would extend into text
  (`SKS`/`E4`/`CW`/etc. at gap end). Sizes are forced by unconditional impl
  reads and the text is genuine, so the bytes cannot be both; static analysis
  cannot distinguish dead code with text-overlapping operands from a
  misidentified op. Needs a runtime fetch trace (dynamic fork, not started).
* 107 gaps with counted ONJS tables larger than the remaining gap, plus 6
  `truncated-onjs-head`. Same verdict as above.
* 19 gaps on path-dependent ops (`GP`/`IF`/`IH`/`CALC`/`HP`) whose operand
  count is runtime-selected; `KIDSET`'s conditional u32 (see above).
* 8 single-occurrence unknown ops (`36`, `76`, `86`, `16`, `17`, `BE`, `F2`-adjacents…;
  each blocks exactly one gap).
* Which op displays text in scenes like `A0_0.ISF` -- it contains **zero**
  `0x2B` bytes (OBSERVED), so its text path is not `PM`; the per-message
  trailer is `[lone,00][ED][ONJS …]` in 20 068/20 115 cases but the display
  call itself is unidentified.

## 5. `NURSEUS.SUF`, `NURSEUS.SAV`, `NURSEUS.HLP`

* `NURSEUS.SUF` (91 B, plaintext INI), read/written through `GetPrivateProfileStringA` /
  `WritePrivateProfileStringA`:
  `[StartUpInfo] Company=AngelSmile / Key=NurseUS / Title=Private Nurse / HelpName=NURSEUS.HLP`.
* `NURSEUS.SAV` (64 246 B): **binary, no ASCII sentences at all** (E03) — the longest strings found
  were 0 words. Layout **UNKNOWN (U3)**; the name *is* a literal in the EXE (E10).
* `NURSEUS.HLP` (713 016 B): WinHelp manual, the only file with long real English prose besides the
  EXE's own messages (E03). Opened via `WinHelpA`.

## 6. Tool → format map

| Format | Reader | Writer | Self-test |
|---|---|---|---|
| SM2MPX10 | `tools/unpack.py: read_sm2mpx` | — | `--selftest` (align16 first payload, zero pad, contiguity, 16-byte alignment, EOF) |
| SE/VOICE/WMSC | `tools/unpack.py: read_table` (16 / 32-byte records) | — | `--selftest` (first start == table end, strictly increasing, EOF marker / `endOffset`, `RIFF`+`WAVE` magic on every payload) |
| ISF text | `tools/decode_isf_text.py: text_runs, char, all_lines` | `--dump` writes plain text | `--selftest` (4 sentences + totals + class boundary + unmapped set + `0xFD` budget) |
| Archive extract | `tools/unpack.py: extract` | writes payloads | `--selftest` extracts all five archives; ISF/SE/WMSC compared byte-for-byte |
| GGD/GAD pixels | `tools/decode_ggd.py: decode` | `--png` writes PNG | `--selftest` decodes all 1036 entries incl. GAD sheets (E17) |
| ISF bytecode | `tools/probe_isf_opcodes.py` (op→handler/stub/impl) + `tools/walk_isf.py` (operand table + gap tiler + `--scene` disassembler) | `--scan` in E12 (19 399/20 115 gaps tile, 12 234 with TAG; 716 blocked with per-reason counts); ground-truth re-disassembly of every encoded impl; `--scene` asserts full-file coverage (332 scenes, 0 failures) |
