# 01 — The stack

Component view, with the evidence for each cell and its behaviour on a current Windows 11 (x64)
machine. Tags: OBSERVED (E-id), INFERRED, UNKNOWN. Full citations in
[`00-scouting-report.md`](00-scouting-report.md); regenerate evidence with
`bash tools/regen_evidence.sh`.

## 1. Stack table

| Layer | What it actually is | Evidence | Windows 11 status | Risk |
|---|---|---|---|---|
| Executable | PE32 x86, VC6 linker 6.0, 430 373 B, unpacked (`.text` entropy 6.51), timestamp 2002-10-22 | E01, E00 | Runs under WOW64 | Low |
| Manifest | **none** (no `RT_MANIFEST`, `DllCharacteristics 0`) | E02 | Process is DPI-unaware → OS bitmap-stretch on scaled displays; shim engine free to intervene | Medium — user-visible blur |
| Windowing | `CreateWindowExA`, `SetWindowPos`, `MoveWindow`, `ShowWindow`, `ChangeDisplaySettingsA`, `EnumDisplaySettingsA`, `ClipCursor`, `SetWindowsHookExA`, `SHAppBarMessage`, 43 dialog templates, 4 menus, 1 accelerator table | E02 | Works; behaviour on multi-monitor / mixed DPI is the game's own 2002 logic | Medium |
| Graphics | **GDI**: `CreateDIBSection` + `SetDIBColorTable` + `BitBlt`/`StretchBlt` + `SetStretchBltMode`, `CreateCompatibleDC`, brushes/regions | E02 §GDI32 | Works; software blit is the performance ceiling at 640×480 | Low (correct), Medium (quality) |
| Text | GDI `CreateFontA` + `TextOutA`/`ExtTextOutA`/`DrawTextA`, faces referenced: `MS Sans Serif`, `System` | E02, E15 | Works | Low |
| 3D / DDraw | **absent** — no `d3d*`/`ddraw`/`opengl` import or string | E02, E16 | n/a | The `d3d8/d3d9/d3d10core/d3d11/dxgi.dll` (DXVK 10.0.17763.1, 2025-08-30) in the folder intercept nothing |
| Sound effects / voice | `DSOUND.dll ord#1` (`DirectSoundCreate`, INFERRED) + `MSACM32` ACM streams (ADPCM decode) + `WINMM waveOutGetDevCapsA` | E01, E02 | DirectSound still present (emulated); MS ADPCM codec still shipped | Low |
| Music / CD audio | `WINMM mciSendCommandA` + `mixer*` + `mmio*` + `timeSetEvent`, **routed through `fakecdda.dll`** | E01, E02 | Works *because* of the shim; without it the EXE fails to load | High if the shim is dropped |
| Shim chain | `NURSEUS.EXE → fakecdda.dll (mciSendCommandA, mixerSetControlDetails) → fakemm.dll → WINMM.DLL`; tracks `TK02..TK15.mp3` | E01, E00 | Load-bearing 2009 code (8 KB + 5.6 KB, C runtime `msvcrt`) | High — must be kept or reimplemented |
| Assets | 5 archives `GGD` `ISF` `SE` `VOICE` `WMSC`, opened by name from the working directory | E08, E10 | Fine | Low |
| Dialogue assets | compiled scenario: 20 115 lines / 737 989 chars, encoding solved; bytecode 96.4% tiled + message tags | E05, E13, E14, E12 | Low |
| Save/config | `NURSEUS.SAV` (64 246 B binary), `NURSEUS.SUF` (INI via `GetPrivateProfileStringA`/`WritePrivateProfileStringA`) | E03, E02 | Fine; layout UNKNOWN | Medium (blocks a clean-room reimplementation) |
| Registry / install | `ADVAPI32 Reg*`; an engine message about incorrect registration (counted in E03) | E02, E01 | Registry path assembled at runtime (not a plain string, E09) → check on a clean VM | Medium |
| Help | `USER32 WinHelpA` + `NURSEUS.HLP` (713 KB) | E02, E15, E00 | **INFERRED:** WinHelp is not installed by default on Win10/11 → in-game help fails | Low (cosmetic) |
| Other | `WINSPOOL OpenPrinterA/DocumentPropertiesA/ClosePrinter` (printing), `COMCTL32 PropertySheetA` (options dialog), `comdlg32 GetFileTitleA`, `ole32 CoInitialize/CoCreateInstance` | E02 | Works | Low |
| Safety posture | VC6 C++ with `IsBadReadPtr`/`IsBadWritePtr`/`GlobalHandle`-era idioms; no ASLR/NX flags (`DllCharacteristics 0`) | E02 | Engine bugs are unfixable without patching or replacing the engine | Inherent |

## 2. What actually breaks on a modern machine (ranked, with cause)

1. **Hard load-time dependency on two 2009 DLLs** (F04). If `fakecdda.dll`/`fakemm.dll` are missing or
   blocked, the EXE never starts: the loader fails on the import descriptor. Any redistribution of a
   fixed build must ship them, or the imports must be re-pointed at a re-implementation.
2. **DPI blur / window geometry** (F05): no manifest → DPI-unaware → Windows stretches the result.
   Cheapest candidate fix is an external manifest, no code — but it is **INFERRED and untested**;
   try it and measure, don't assume it.
3. **WinHelp help button dead** (F07): needs the WinHelp redistributable or a help replacement.
4. **ANSI-only path handling** (F06): non-ASCII or very long install directories break asset loading.
5. **Registration/install check** (F14): may fail outside the original installer flow — must be
   verified on a clean VM before claiming it works.
6. **CD-audio-era behaviour** (F03/F04): volume control goes through `mixerSetControlDetails` (legacy
   MME mixer); on machines without a legacy mixer node this silently no-ops.
7. **Quality/scaling** (user's complaint): art is ~24-bit 640×480 (F10) blitted by `StretchBlt` with a
   palette-capable DIB, and text is a GDI system font (F16). Nothing here is broken; it is simply
   2002 output. This is the one problem that config cannot fix — it needs either a patched blit path
   or a replacement renderer.

## 3. Architectural picture (one screen)

```
NURSEUS.EXE (Win32, GDI + DirectSound + MME)
├─ windowing: CreateWindowExA / ChangeDisplaySettingsA / SetPosWindow … (2nd window claim UNKNOWN, F15)
├─ graphics: 8-bit-capable DIB (CreateDIBSection/SetDIBColorTable) → StretchBlt to the window
│             images come from GGD (tag at +0) / GAD (magic "GAD ", tag at +86 or +0x116):
│             0xB9AAB3B3 + w/h + RLE/delta pixels (~24-bit, F10)
├─ text:     code stream 0xFD00..0xFDFF → in-engine table → GDI font (F12, F16)
│             + 1-2 byte message tag before quoted lines (02-formats.md §4.2)
├─ audio:    DirectSound + MSACM (ADPCM) + WINMM(mci/waveOut/mixer/mmio) → fakecdda → fakemm → WINMM
├─ data:     GGD (1036 img) ISF (803 scen) SE (77) VOICE (2715) WMSC (1)
├─ state:    NURSEUS.SAV (64 246 B binary) + NURSEUS.SUF (INI) + registry registration key
└─ help:     WinHelpA → NURSEUS.HLP (removed from modern Windows)
```

## 4. What this means for "moving the graphical stack"

There is no Direct3D/DDraw stack to move: the graphical surface is GDI + a DIB + `StretchBlt`. The
choices in [`03-plan.md`](03-plan.md) are therefore (a) leave the renderer alone and fix the
*environment* (manifest, windowing shim, DLL replacement), or (b) intercept the blit and re-present
it (scaling/filtering), or (c) replace the engine entirely (needs U2/U3 first). "rust-sdl2" is only
relevant to (b) and (c); for (b) a Rust DLL that hooks GDI/`CreateWindowExA` is a much smaller
first step than an SDL2 rebuild of the game's front end.
