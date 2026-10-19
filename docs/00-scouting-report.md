# 00 — Scouting report

Scope of this session per the brief: *"a scouting reverse engineering run done for the EXE and related
files to establish the 'stack'"*, plus "scaffolding setup for future agent sessions".
No binary was executed, no runtime instrumentation was used; everything below is static, from the
shipped files. Claim tags: OBSERVED / INFERRED / UNKNOWN (see [`README.md`](README.md)).
Reproduce all evidence with `bash tools/regen_evidence.sh` (inputs pinned in `E00`).

## 1. One-paragraph answer

This is AngelSmile's in-house 2002 engine: a 32-bit MSVC6 Win32 program that draws with **GDI**
(palette-capable DIB sections blitted with `StretchBlt`, text through `TextOutA`/`ExtTextOutA`/
`DrawTextA` with a `CreateFontA` system font), plays audio through **DirectSound** (1 ordinal),
**WINMM** (`waveOut`/`mci`/`mmio`/`mixer`/`timeSetEvent`) and **MSACM32** (ADPCM decode), and reads
five custom archives. There is **no Direct3D and no DirectDraw** — the DXVK DLLs sitting in the game
folder cannot affect it. It has **no embedded manifest**, so Windows applies DPI virtualisation and
compat heuristics. Two of its imports come from **`fakecdda.dll`**, a 2009 third-party CD-audio→MP3
shim, so those DLLs are hard link-time requirements, not optional add-ons. The good news: the game's
**complete dialogue text (~718 k characters, 20 114 lines) is extractable**, because the character
encoding inside the compiled scenario files is now solved.

## 2. Assumptions in the brief, checked

| Brief claim | Verdict | Basis |
|---|---|---|
| "old visual novel" | TRUE — engine + launcher date to 2002 (`NURSEUS.EXE` PE `TimeDateStamp` 1035293042 (Unix epoch) = 2002-10-22 13:24:02 UTC, VC6 linker 6.0) | E01 |
| "does not even run without third-party DLLs" | TRUE by construction — the EXE statically imports `mciSendCommandA` and `mixerSetControlDetails` **from `fakecdda.dll`**, which in turn needs `fakemm.dll` | E01 (`IMPORT fakecdda.dll`), E02 |
| "compatibility issues on Windows 11" | TRUE, root causes identified below (no manifest, DPI, WinHelp, ANSI paths, load-time DLL requirement) | E02, E15 |
| "Resolution is fixed with no windowing, borderless, scaling" | PARTLY TRUE — the window/display mode is chosen by the game (`ChangeDisplaySettingsA`, `EnumDisplaySettingsA`, `CreateWindowExA`, `SetWindowPos` are imported) and the *blit* is a `StretchBlt`, so in-engine scaling is technically already available; "fixed" is a policy of the game, not a limit of the API | E02 §GDI32/USER32 |
| "its dialogue text box is a borderless window, which glitches" | **UNVERIFIED** — plausible (the engine is full of window juggling) but this session found no evidence for or against. Needs a runtime window trace (see §5 U4) | — |
| "alt+tab is not handled gracefully" | **UNVERIFIED** (no runtime observation this session) | — |
| "move the graphical stack to rust-sdl2 / rewrite" | Premise corrected: there is no 3D stack to move. Options in [`03-plan.md`](03-plan.md); the ladder starts at config-only fixes because the engine is GDI + a 640×480 24-bit art pipeline | 01-stack.md |
| "running it leads to compatibility issues, memory safety is a concern" | The EXE is VC6-era C++ with `IsBadReadPtr`/`IsBadWritePtr` idioms; our safety only matters in *new* code unless we do a full rewrite | E02 |

## 3. Findings

### F01 — Engine identity and build
**OBSERVED.** PE32 (machine `0x014c`), GUI subsystem 2, linker 6.0 (VC6), `.text` entropy 6.51 (no
packer), sections `.text/.rdata/.data/.rsrc/.imports`, no debug directory, `DllCharacteristics 0`.
Version resource: `CompanyName=GamelexGS Inc.`, `LegalCopyright=© 2002 AngelSmile © 2002 GamelexGS
Inc.`, `FileVersion 1.0.0.1`. `NURSEUS.SUF` = `[StartUpInfo] Company=AngelSmile / Key=NurseUS /
Title=Private Nurse / HelpName=NURSEUS.HLP`.
Evidence: E01 §NURSEUS.EXE, E02 (§resources, §DllCharacteristics, §debug), E00.

### F02 — Rendering is GDI, not Direct3D/DirectDraw
**OBSERVED.** Imports: `GDI32: BitBlt CreateDIBSection CreateCompatibleDC CreateFontA Escape
ExtTextOutA PatBlt SetDIBColorTable SetStretchBltMode StretchBlt TextOutA ...`; `USER32: BeginPaint
DrawTextA EndPaint FillRect GetDC ...`. Import descriptors contain no `d3d*`, `ddraw`, `opengl`, and
no such DLL name string exists in the image (E16).
**INFERRED.** The `SetDIBColorTable` + palettised-DIB combination suggests an 8-bit offscreen buffer
whose palette is set per scene; art assets themselves are ~24-bit (F10). The offscreen buffer width
should be confirmed by a runtime trace.
**Consequence.** DXVK (`d3d8/d3d9/d3d10core/d3d11/dxgi.dll`, all version-stamped "DXVK 10.0.17763.1",
dated 2025-08-30) has nothing to intercept. Removing it is risk-free and removes GPU-driver surface.

### F03 — Audio stack
**OBSERVED.** `DSOUND.dll ord#1` (ordinal 1 of DSOUND is `DirectSoundCreate` — INFERRED naming from
the standard DSOUND export order), `WINMM.dll` 24 imports (`waveOutGetDevCapsA`, `mciSendCommandA`,
`mixerOpen/mixerGetLineInfoA/mixerSetControlDetails`, `mmio*`, `timeSetEvent/timeBeginPeriod`),
`MSACM32.dll` 14 imports (`acmStreamOpen/Convert/...`).
**INFERRED.** CD-DA playback via MCI was replaced by MP3 playback by the shim (F04); the MP3 tracks
are the top-level `TK02.mp3`…`TK15.mp3` (14 files, E00). Voice/SE files are ADPCM WAV (F09).

### F04 — The third-party DLL chain is a hard dependency
**OBSERVED.** `NURSEUS.EXE` import descriptors: `fakecdda.dll` → `mciSendCommandA`, and a second
descriptor `fakecdda.dll` → `mixerSetControlDetails`. `fakecdda.dll` (8 192 B, "FakeCDDA 0.2.2",
"Vegard Fiksdal Technologies (C) 2009", FileDescription "Changes from CDDA playback to mp3") exports
`mciSendCommandA/mciSendStringA/mixerSetControlDetails` and imports `WINMM_mciSendCommandA`,
`WINMM_mciSendStringA`, `WINMM_mixerSetControlDetails` **from `fakemm.dll`**. `fakemm.dll` (5 632 B,
"FakeCDDA 0.2.0") imports the real `mciSendCommandA/mciSendStringA/mixerSetControlDetails` from
`WINMM.DLL` and re-exports them under the `WINMM_` prefix.
**Consequence.** The engine's own CD-audio calls are unmodified upstream calls; the shim is
load-bearing. Any modernisation that keeps the EXE must keep both DLLs (or reimplement them).

### F05 — No manifest: DPI virtualisation and compat heuristics apply
**OBSERVED.** No `RT_MANIFEST` resource (E02 §RT_MANIFEST prints `(none)`), `DllCharacteristics 0`.
**INFERRED.** Without a manifest the process is DPI-unaware, so on a scaled display Windows
bitmap-stretches the whole window; and the "installer detection"/shim layer gets to make choices.
**Cheapest fix, no code (INFERRED, untested):** ship an external `NURSEUS.EXE.manifest` beside
the EXE declaring `dpiAware`. The two MS pages cited in §6 describe manifests and DPI defaults
but **do not** establish the `<exe>.manifest` external-lookup precedence this depends on, and
since Windows 10 1607 external manifests are honoured only for executables that create an
activation context — which a 2002 VC6 binary may not. This is the cheapest candidate fix and it
must be *tried* (M0 step 2), not assumed.

### F06 — ANSI-only file APIs and long paths
**OBSERVED.** Every filesystem call is the `…A` variant (`CreateFileA`, `FindFirstFileA`,
`GetFullPathNameA`, `SetCurrentDirectoryA`, `GetPrivateProfileStringA`, …) — E02 §KERNEL32.
**INFERRED.** Install paths containing non-ANSI characters (or >260 chars) are unsupported by the
engine. This is a real "modern system" failure mode that no GPU setting fixes.

### F07 — In-game help uses WinHelp
**OBSERVED.** `USER32: WinHelpA` is imported; `NURSEUS.HLP` (713 016 B) is a WinHelp file and
`NURSEUS.SUF` points at it (`HelpName=NURSEUS.HLP`). E15/E02.
**INFERRED.** Windows 10/11 ship without `WinHlp32.exe`, so the in-game help button fails unless the
user installs Microsoft's WinHelp redistributable. Verify by pressing Help once at runtime.

### F08 — Archives: shape and sizes
**OBSERVED** (verified parsers, `--selftest` asserts every invariant): 

| File | Format | Entries | Payload bytes | First / last entry |
|---|---|---|---|---|
| `GGD` | SM2MPX10 | 1036 | 343 631 962 | `AFTER.GGD` / `YU_007.GGD` |
| `ISF` | SM2MPX10 | 803 | 2 618 990 | `-C-O\x99\xC5.TXT` / `TU_END3.ISF` |
| `SE` | u16 table | 77 | 9 922 994 | `AY_0412.WAV` / `TA_0055.WAV` |
| `VOICE` | u16 table | 2715 | 136 126 808 | `AO_0001.WAV` / `YU_0035.WAV` |
| `WMSC` | u16 table (32-byte records) | 1 | 352 940 | `MUON.WAV` |

Evidence: E08 (`GGD entries=1036 payload_bytes=…`), E02.

### F09 — Names are opened by string, and the engine's own message catalogue
**OBSERVED.** NUL-terminated strings in the EXE: `GGD`, `ISF`, `VOICE`, `WMSC`, `MUON.WAV`,
`NURSEUS.SAV`, `NURSEUS.SUF`, `.GAD` (E10). `SE` is also present in the image but E10 cannot
show it: that evidence is produced with `strings -n 3`, which structurally cannot emit a
2-character string. Engine messages: six strings about missing DirectSound, missing ADPCM
support, failed registration, a compile failure, a fade-in memory failure, and a display
colour failure (E15 for the first two; E03 for the rest — `pe_report.py` emits
headers/imports/exports/version only, so it is **not** the source for these strings). The
wording is the game's own and is not reproduced in this repository.
**INFERRED.** The five archives are opened by name from the working directory, so the data files
must stay next to the EXE (or be redirected by hooking the A-API path resolution).

### F10 — GGD/GAD are image containers; art is ~24-bit with a delta/RLE codec
**OBSERVED.** Two inner layouts. `GGD` payloads begin `B9 AA B3 B3` + `u16 width; u16 height` at
offset 0 (1 032 entries). The four `.GAD` animation sheets instead begin with the ASCII magic
`"GAD "`, followed by a variable-length header, then the same tag — at offset **278** for
`ANGEL.GAD` (1440×720), `ANIME.GAD` (1440×720) and `CDF08.GAD` (160×1280), but at offset **86**
for `ANGEL3.GAD` (360×720, 235 325 B), whose header opens `47 41 44 20 | 56 00 00 00 | 01 00
00 00 | …` (E06; tag-offset histogram `{0: 1032, 86: 1, 278: 3}`). Every entry carries exactly
one tag (E06). `(payload − tag_offset − 8) / (w*h)` spans **0.008** (`BLACK.GGD`, 2 422 B,
640×480) to **3.125** (`ETC07_13.GGD`, 959 864 B, 640×480) (E07).
**INFERRED.** A bare 24 B/px raw buffer caps at exactly 3.00 B/px, so 3.125 B/px requires either
a raw form above 3 B/px or ≥ 38264 B (3.99 %) of per-entry overhead — and the known header is
8 bytes, making that unlikely but not excluded. 4 B/px (32-bit) with an RLE/delta pass in front
is therefore the most plausible reading: noisy scenes then run at 0.781 of raw, flat scenes at
~0.8 %. Art is ~24-bit (the 8-bit-DIB `SetDIBColorTable` path in F02 is the offscreen buffer,
not the asset format).
**SUPERCEDED — codec solved (U1 closed 2026-10-18).** Op stream at payload+8, opcodes
recovered from the EXE's own dispatch (`0x40827d`, table `0x408438`): `0x00 N` = N copies
of previous triplet; `0x01 L O` / `0x02 L OO` = copy L triplets from O back;
`0x03 O` / `0x04 OO` = copy 1 triplet from O back; `0x05`–`0xFF` = (op−4) literal RGB
triplets. `tools/decode_ggd.py --selftest` decodes all 1036 entries (E17); renders
verified (`AFTER.GGD` title screen legible, GAD sheets intact). Full table in
[`02-formats.md`](02-formats.md) §3. The ≥3.00 B/px ratios are opcode overhead on
3.00 B/px of colour, not 32-bit pixels.
**UNKNOWN (remaining):** what the `GAD ` header fields (tag offset, image count, the u16
pairs) encode, and why `ANGEL3.GAD` uses an 86-byte header rather than 278.

### F11 — ISF is the compiled scenario, and it was built from English source files
**OBSERVED.** `ISF` contains 801 `*.ISF` scenario objects plus two build artefacts:
a Shift-JIS developer note `-C-O<sjis>.TXT` (1 095 B) about half-width character handling
that documents four script command mnemonics and quote escaping, and `ERROR.LOG` (100 102 B)
which lists **801 absolute source paths** of the form `<build share>\<release>\<scene>.txt`,
each followed by a Japanese conversion status line (E11, E12, E05). The developer text is
not reproduced here.
**INFERRED.** The English release's dialogue was compiled from `.txt` sources by a converter that
lives in (or with) `NURSEUS.EXE` — the EXE contains a compile-failure message (E03)
and `Compile`-related strings, and the doc file is written in the same Japanese as the
converter's status messages. The 801 source paths are also a free map of the game's scene structure.

### F12 — The dialogue text is fully extractable (encoding solved)
**OBSERVED.** Inside ISF, dialogue is a byte stream of `<code> 0xFD` pairs: each character is
`code = 4*(ascii & 0x3F) | class`, with `class = 1` for letters and `class = 0` for
space/punctuation/digits; `0xFF` (`0xFDFF`) marks the start of a message and never appears inside
one; codes with `code & 3` equal to 2 or 3 never occur as characters. Decoding all 803 entries
yields **737 989 characters in 20 115 message runs with zero unmapped codes**, and every `0xFD`
byte in the payload region is accounted for by the pairing (E05: 359 853 at even entry-relative
offsets, 358 207 at odd). Runs are *not* word-aligned (markers split 10 028 even vs 10 087 odd,
E05), so this is a byte-pair stream, not a `u16` stream. Each message ends with a lone
terminator code + `0x00` (final punctuation without its `0xFD` marker — see `02-formats.md`
§4.1); that closes the old unbalanced-quote question below.
Samples: four messages decode at known offsets in `A0_0.ISF` and `A0_2.ISF` (E05). The
text is the game's own and is not reproduced here; `tools/decode_isf_text.py` prints
offsets and lengths only.
Evidence: E04 (87 distinct codes in the even-aligned view / **78** across the whole corpus; most
frequent = space, frequency ordering matches English), E05 (decoded counts), E13 (corpus
size as a statistic).
**Residual UNKNOWN (closed — see `02-formats.md` §4.1).** The old "7 030 `"` with no closer"
signature was the 6 867 lone `"` terminators; quotes balance, no third class needed.
**Consequence.** Text is not a blocker for any modernisation lane, and re-encoding for mods is
symmetrical. `GAME.ISF`/`CD.ISF`/`CG_ALL.ISF` carry the system-level scripts.

### F13 — Where the text is *not*
**OBSERVED.** A sentence-shaped plaintext scan over every shipped file finds real English prose only
in `NURSEUS.HLP` (the manual) and the EXE's own message strings; `GGD`/`SE`/`VOICE` hits are
compressed-image/audio noise; `NURSEUS.SAV` has none (E03).
**Consequence.** Any tool that "just greps the data files" fails; the encoding in F12 is the only
route, and it exists.

### F14 — Save/registry/print surface
**OBSERVED.** `ADVAPI32: RegOpenKeyExA RegQueryValueExA RegCreateKeyExA RegSetValueExA
RegCloseKey`; `NURSEUS.SAV` is 64 246 bytes of binary (no ASCII runs, E03);
`KERNEL32: GetPrivateProfileStringA/WritePrivateProfileStringA` for INI-style config;
`WINSPOOL.DRV: OpenPrinterA/DocumentPropertiesA/ClosePrinter` → printing;
`SHELL32: SHAppBarMessage`; `COMCTL32: PropertySheetA CreatePropertySheetPageA
DestroyPropertySheetPageA` + one ordinal (#17) → the options dialog is a property sheet;
`ole32: CoInitialize/CoCreateInstance`; `KERNEL32: CreateProcessA` (launches something
external), `USER32: SetWindowsHookExA`, `ClipCursor`.
**INFERRED.** An engine message about incorrect registration (E03 counts it) plus the
`Reg*` imports indicate an install-time registration key check (the literal registry path is not
stored as a plain string — E09 — so it is assembled at runtime).
**UNKNOWN.** `NURSEUS.SAV` layout, and whether the registration check can fail on a modern machine
(registry virtualisation) — testable by running the game without the installer.

### F15 — Second window / alt-tab claims are unverified
**UNKNOWN.** No static evidence was collected about how many top-level windows the game creates or
their styles. `CreateWindowExA`, `SetWindowPos`, `ShowWindow`, `MoveWindow`, `SetWindowLongA`,
`SetWindowsHookExA`, `SHAppBarMessage` are all imported, so window juggling is certainly present, but
"the text box is its own borderless window" must be confirmed with a runtime capture (see U4).

### F16 — Font: GDI system font for text, no font file in the data
**OBSERVED.** `GDI32: CreateFontA`, `USER32: DrawTextA`, `GDI32: TextOutA/ExtTextOutA/GetTextExtentPoint32A`;
font-face strings in the EXE: `MS Sans Serif`, `System`, plus MFC leftovers (`CFont`,
`AU_AFX_PROPPAGEFONTINFO`) (E15). No `.ttf`/`.fon` exists anywhere in the shipped data (E00 file list).
**INFERRED.** Text is drawn with a Windows font, and the `0xFDxx` codes are mapped to characters by an
in-engine table (the structure F12 recovered). The exact face and size per UI element still needs a
runtime check; it matters for the "remaster" lane, because text quality is then a font choice, not an
asset problem.

## 4. Stack summary
See [`01-stack.md`](01-stack.md) for the table with per-component Windows 11 status and risks.

## 5. Open unknowns (each with the cheapest thing that would close it)

1. **U1 — GGD image codec: CLOSED 2026-10-18.** Op table recovered from the EXE
   (`0x40827d`, jump table `0x408438`); `tools/decode_ggd.py --selftest` decodes
   all 1036 entries incl. the 4 GAD sheets (E17); renders verified. See
   [`02-formats.md`](02-formats.md) §3.
2. **U2 — ISF bytecode structure (ISA recovered 2026-10-18, handlers open).** Opcode
   table (117 ops `0x00 ED`–`0xFF EXT` with Japanese descriptions, E18) recovered from the EXE;
   VM loop at `0x424e30` (op byte + 1-or-2-byte prefix, dispatch via map `0x42576c` into
   136-entry table `0x42554c`). Still open: per-op operand sizes beyond the prefix (136
   handler bodies at `0x426xxx`–`0x427xxx`), then a walker that disassembles one scene end
   to end.
3. **U3 — `NURSEUS.SAV` layout** (64 246 B). Diff two saves taken one scene apart; the engine's save
   code is reached from the property-sheet/menu handlers.
4. **U4 — window model / text box** (F15, and the alt-tab complaint). Runtime capture of
   `CreateWindowExA`/`SetWindowPos` calls with class, style, parent and rect while a scene plays and
   while alt-tabbing.
5. **U5 — registration check** (F14). Run the game with no registry key present on a clean VM.
6. **U6 — GAD animation sheets** (header **parsed**, semantics open). `GAD ` magic, tag-offset
   field, image count (= 1), 4-u16 prologue, frame records `(S,0,0,150)` / `(0,BASE,k·S,150)`
   + trailing zero — all asserted by `tools/probe_gad_header.py` (E06). ANGEL3's 8 records are
   ANGEL's first group verbatim; ANIME = ANGEL with BASE+1 in two groups and zeroed in the last.
   Still UNKNOWN: prologue fields, record semantics, BASE meaning. Next: find the EXE code that
   walks them (start from the callers of the `GAD` name string). Pixel payloads of
   all 4 sheets now decode with the U1 codec (viewed: ANGEL 32-frame logo fade,
   ANGEL3 8-frame column); only the header/record semantics stay open.
7. **U7 — does the EXE expose its `.txt → .ISF` converter as a command line?** (`UNKNOWN`; if yes, it
   makes modding/re-localisation trivial. Static half-answer: `GetCommandLineA` **is** imported
   from KERNEL32 (E02), so argv is read *somewhere*. Still UNKNOWN whether any argv reaches the
   compile path — that needs disassembly/data-flow from the `GetCommandLineA` call sites, not
   more string searching.)

## 6. References used for the plan

* External manifests (fixing DPI awareness without touching the EXE):
  <https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests> (HTTP 200, checked)
* DPI awareness defaults: <https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process> (HTTP 200, checked)
* `minhook` (x86 detour library, MIT): <https://github.com/TsudaKageyu/minhook> (HTTP 200)
* `rust-sdl2`: <https://github.com/Rust-SDL2/rust-sdl2> (HTTP 200); alternatives
  `rust_minifb` <https://github.com/emoon/rust_minifb>, `pixels` <https://github.com/parasyte/pixels>
* DXVK (confirmed irrelevant here, listed so reviewers can verify the claim)
  <https://github.com/doitsujin/dxvk>

**Evidence / citation note.** All URLs in §6 were re-checked and return HTTP 200. Two of them
are annotated "(HTTP 200, checked)" but neither substantiates the sentence it is attached to:
the manifest page does not cover external-manifest lookup (see F05), and the WinHelp claim
below has no link at all.

WinHelp removal on Windows 10+ is asserted from operating-system behaviour, not a link; verify by
invoking the in-game Help once (F07).
