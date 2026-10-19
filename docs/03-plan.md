# 03 — Modernisation plan

The brief asked for rust-sdl2 (or equivalent) and a possible rewrite. Scouting changed the shape of
that request — see [`00-scouting-report.md`](00-scouting-report.md) §2 for the assumption-by-assumption
check. Short version: there is **no 3D stack to port** (F02: GDI + `StretchBlt` + `CreateDIBSection`),
the assets and the text are now largely understood (F08, F10, F12), and the scenario bytecode (U2) and
save format (U3) are the two things that make a clean-room engine expensive.

## 1. The ladder (each rung is independently shippable)

| Rung | What it is | Effort | Fixes | Leaves broken |
|---|---|---|---|---|
| **L0** Config only | delete the inert DXVK DLLs; add an external `NURSEUS.EXE.manifest` (dpiAware); ship the shim DLLs + WinHelp redistributable next to the EXE; short ASCII path | hours | DPI blur, dead help button, wasted GPU surface, first-run surprises | alt-tab, text-box glitch (if real), no scaling |
| **L1** Loader shim | small 32-bit DLL that redirects the ANSI file APIs so assets can live in a modern location / long path / non-ASCII name; or just co-locate files | 1–2 days | path handling (F06), distribution layout | everything else |
| **L2** Presentation shim (Rust) | 32-bit `fakecdda.dll` proxy loads a presenter that stays in GDI: suppress the mode switch, own the window size/style, scale the engine's 640×480 frame into the game's own window, remap mouse geometry and fix alt-tab. Build spec, milestones and traps: [`04-l2-presenter.md`](04-l2-presenter.md). SDL2 is the *escalation* rung, not the default (`04` §4) | ~1–2 weeks (phases A–D, `04` §6) | windowing, scaling quality, alt-tab, resampling, DPI blur, wasted GPU surfaces | the engine's own logic, save format, script VM |
| **L3** Hybrid engine | reimplement the front end on the original archives: GGD codec (U1), ISF VM (U2), audio, save (U3); reuse the decoded text corpus | 1–3 months | everything, incl. cross-platform and high-res rewrite | — |
| **L4** Full replacement | own data format, own script language, port assets once | months+ | total control, moddability | save compatibility (unless U3 is solved) |

**Recommendation: L0 now, L2 as the target, L3 only if U1/U2/U3 land.** L2 is where the
presentation shim earns its place — as the *presenter*, not as a re-implementation of the game.
The agreed design is GDI-in-place (no second window); SDL2 enters only through the escalation
triggers in [`04-l2-presenter.md`](04-l2-presenter.md) §4. L1 (file-API redirect) is not required by
that deliverable — the zip is copied into the game folder, so assets stay co-located; it remains the
fallback for a game installed under a non-ASCII or over-long path (F06).

Honest cost note: L2's risky part is that the game draws into its own DIB and blits it; we must
intercept at `BitBlt`/`StretchBlt` time (or take the DIB with `GetDIBits`) and know which surface is
"the frame". M0 step 3 went some way (`s8_handoff.md` §5: one metronomic 640×480 `BitBlt` per frame
at dialogue, text composited as bitmaps, no second window — **single-run data**), and L2-M1
re-measures it per phase (`04` §6). The ~1–2 week figure assumes the spike (phase A) confirms that
shape; it grows if the frame turns out to be composited from several DCs.

## 2. Recommended first milestone (M0, this week, no RE required)

M0 works on a **staging copy**, never on `original/` — `AGENTS.md` declares `original/` pristine,
and `tools/regen_evidence.sh` reads it (pefile over the five D3D DLLs, sha256 of all 32 files
into E00/E01). Copy `original/` to a scratch directory and run the game from there.

1. Stage `original/` to e.g. `work/run/`, then in the staging copy remove the inert DXVK DLLs
   (`d3d8/d3d9/d3d10core/d3d11/dxgi.dll` — no D3D import anywhere in the EXE, F02). Cost of
   reversal is zero because `original/` was never touched.
2. In the staging copy, add `NURSEUS.EXE.manifest` declaring `dpiAware`/`dpiAwareness=PerMonitorV1`
   and measure whether the blur is gone at 150 % scaling. Treat this as a **hypothesis to test**,
   not a known fix (F05).
3. Confirm the game still starts (the shim DLLs are load-bearing — F04) and note the exact
   behaviour on this machine: window style, alt-tab, help button, registration check.
   **Also capture the present blit**: hook or trace `StretchBlt`/`BitBlt` destination DCs and
   record which surface is the frame — §5 lists this as risk #1 and M0 is the only cheap moment
   to settle it (it is what decides whether L2 is one week or three).
4. Diff two saves taken one scene apart (U3 groundwork).

**Acceptance for M0:** the game starts and runs at 150 % display scaling with OS upscaling instead of
GDI stretch, `original/` is byte-identical to E00 afterwards, and we have a written list of the
*observed* symptoms (not assumed ones) — the two symptoms F15 flags as unverified get verified or
struck, and the `StretchBlt` destination is recorded.

## 3. Second milestone (M1): the two unknowns that gate everything else

*(Naming trap: this M1 is an RE milestone. `docs/04-l2-presenter.md` runs its own series `L2-M0…M4`
for the presenter build — the two are unrelated.)*

* **U1 image codec** — CLOSED (see above; `tools/decode_ggd.py` + E17).
* **U2 scenario VM** — static part CLOSED, dynamic part open. Operand table for
  ~110 ops recovered from per-impl disassembly (ground-truth re-checked;
  `tools/walk_isf.py: EXTRA` + `--scan` in E12: 19 399/20 115 gaps tile,
  332 scenes disassemble end to end via `--scene`). Open: ~582 operand
  overruns + 107 ONJS-table overruns (verified ops whose operands would
  extend into genuine text -- needs a runtime fetch trace to distinguish dead
  code from misidentified ops), 19 path-dependent ops (`GP`/`IF`/`IH`/`CALC`/
  `HP`), 8 singletons, the text-display op, and all jump *targets*
  (state-dependent via `0x425870`). U2 acceptance (one scene end to end,
  statically) is met by `A_H11.ISF` (107 gaps).

Both are *static* work on `NURSEUS.EXE`; neither needs the game to be running. U2 also answers U4
(window model) if the same pass traces `CreateWindowExA` call sites.

## 4. Open decisions 

D1–D5 are **closed** as of session 10 (L2 grill, 2026-10-18); the reasoning is in the build spec
[`04-l2-presenter.md`](04-l2-presenter.md). What remains genuinely open is listed after the table.

1. **Which rung is the goal?** — CLOSED: **L2**, keep the engine and fix the shell; L3 only if
   U1/U2/U3 land quickly. Loader = proxy `fakecdda.dll`; presenter = GDI in place (`04` §2–§3).
2. **Is the "separate borderless dialogue window / alt-tab glitch" reproducible?** — CLOSED *as a
   design question*: it is measured by the L2-M1 window census per phase (`04` §6 L2-M0 item 2),
   which closes the F15/U4 scope for L2. It is not a user decision and no longer blocks design.
3. **Must `NURSEUS.SAV` load in the replacement?** — CLOSED for v1: a save reset is accepted, so
   U3 is optional and parked (`s1_deferred.md` session-10 block).
4. **Target presentation** — CLOSED: output-side scaling only (internal 640×480 fixed); windowed =
   OS-decorated with a computed integer scale, fullscreen = borderless aspect-fit smooth; setting
   list in `04` §5 (tentative).
5. **Stance on the original EXE** — CLOSED: keep and patch the original engine; the release ships
   only our DLL/EXE/manifest/README and never redistributes the game or the stock shims (`04` §1, §8).

Still open (user decisions):

* U2 dynamic fetch trace — needs explicit go-ahead; **not** on the L2 path (parked).
* 150 % DPI acceptance case and the `PerMonitorV2` retest (`04` §3, review O3) — tested during L2
  phase A/B, escalated only if the mechanism stays unexplained.
* Resizable windowed mode — tentative, demotes to v1.1 if the engine fights the window (`04` §6).

## 5. Risks to the plan

| Risk | Effect | Mitigation |
|---|---|---|
| The GDI blit is not a single identifiable "present" | L2 hooking point unclear | M0 step 3 got the first answer — one metronomic 640×480 `BitBlt` per frame at dialogue, text composited as bitmaps, no second window (`s8_handoff.md` §5, **single-run**) — and L2-M1 re-measures per phase, including the 16 `StretchBlt` dests never classified (`work/tmp/grid_run11.log:92`). Both fids are hooked from the start (`04` §3, NF-5) |
| Engine-side frames are black (not a presenting bug) | presenter work aimed at the wrong layer | L2-M0 item 3: fresh source-DIB **and** window-DC time series before M2; the old "all captures uniform/black" premise is stale (the session-10 probe frames show the AngelSmile logo at t≈6 s, `04` §6 NF-1). Stop and investigate if the fresh run shows engine-side black with a healthy screen path |
| Hook install races the loader's IAT snap | hooks silently no-op or are overwritten | order-agnostic install ("patch the slot once it points outside the host image"), bounded, logged, fail-open (`04` §3, R1/NF-2); M1 proves hooks fire before the mode switch |
| The engine reads device queries that the suppressed mode switch would have changed | wrong internal resolution/bpp decisions | M1 logs the requested DEVMODE and `GetDeviceCaps`/`GetSystemMetrics` results before the hook inventory is frozen (`04` §3, NF-3) |
| Codec is per-entry variable (some images use different schemes) | U1 grows | `ETC*` vs `BLACK*` vs `*_P.GGD` (sprites) sampled first; sprite sheets may differ from backgrounds |
| Text class bit has a third case for punctuation we mis-mapped | closed: the 6 867 lone `"` are the missing closers — 13 897 `"` total, balanced, 78 codes, zero unmapped (`02-formats.md` §4.1, E05) | — |
| Message-tag tails tile as code under a wrong model | closed as data: 1-2 trailing bytes before a marker that complete no known op are message tags (nonzero ⇒ quoted line, 100%), not ops (`02-formats.md` §4.2, E12) | — |
| The dialogue text box is a second top-level window (§7 of `04`) | the "one window" present model only holds outside dialogue; a presenter that scales the main window alone leaves the text box 1:1 on top | measured in Phase A (NF-12, `s11_phase_a.md`): subclass both windows; how the box follows the scale is the §7 user decision |
| High-rate engine polls cannot be logged per call | 239 MB log in one run, and the logging itself perturbs the game | observer classes are mandatory: low-rate = per-call, high-rate = counted with first-calls samples (`04` §6 NF-16) |
| Sampling the engine's windows with `PrintWindow`/`SendMessage` blocks while the engine is in its non-pumping poll wait | a hung sampler inside the game process, and false "engine frozen" readings | read window DCs with `BitBlt` instead; no message-based sampling in the presenter (`s11_phase_a.md` NF-18) |
| The tracer's open-loop input can walk into the ActiveMovie/video wait or quit the game | a dialogue run costs ~5 min and fails ~half the time (1 of the last 4 runs reached dialogue) | budget 400 s and a fresh `PN_LOG_DIR` per attempt; validation is retry-based until the engine's video wait is understood (`s11_handoff.md` §3, `s11_phase_a.md` §6) |
| Operand overruns / ONJS-table overruns / path-dependent ops | OPEN: 716/20 115 gaps (3.6%) need a runtime fetch trace to resolve (dead code vs misidentified op); static tiling stops rather than consume text | dynamic trace (U2 remainder) |
| Registration check fails on a clean machine | game won't start for users | M0 step 3 documents it; L1/L2 can stub the `Reg*` result |
| Repo hygiene | a review pass found stale "no git repo" claims and unregenerable evidence | `original/` is git-ignored and hash-pinned; every evidence file is rebuilt by one script |

## 6. What would change the recommendation

* If U1/U2/U3 all fall quickly (a good week), L3 becomes attractive and the text corpus (E13) is a
  head start.
* If U2 turns out to be a large VM with embedded data, L2 stays the answer — hook, don't rebuild.
* If the user's actual complaint is only "it looks blurry and alt-tab is ugly", L0+L2 is a
  week of work and a finished product.
