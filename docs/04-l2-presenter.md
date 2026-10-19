# 04 — L2 presenter: design and build spec

Agreed 2026-10-18 (session 10, grilling round); plan-review amendments and the independent
refutation triage applied the same day. Supersedes
the L2 row of [`03-plan.md`](03-plan.md) §1. Claims carry OBSERVED / INFERRED / UNKNOWN tags and
cite `E00`–`E19` or a named artifact; see [`README.md`](README.md).

Goal: a 32-bit `fakecdda.dll` proxy that hooks the game's GDI presentation and a
config EXE, shipped as a zip that is copied into the game folder. Engine, assets
and save format stay untouched.

## 1. Deliverable and end-user flow

* Archive: `pn_modern.dll` (installs as `fakecdda.dll`), `PN-Patch.exe`
  (settings + Install/Uninstall + Play), `README.md`, the licence file. **No manifest**
  (A2, user decision session 14: the DLL sets DPI awareness itself, INI `dpi_aware`,
  default on — §3 DPI).
* Install, manual (README documents these exact steps) or via PN-Patch:
  rename stock `fakecdda.dll` → `fakecdda_real.dll`; drop ours in its place;
  offer to move the five inert DXVK DLLs (`F02`, `E16`) to
  `_backup/`; write the INI. Afterwards `NURSEUS.EXE` double-clicks normally.
* Install guards (review O7): refuse while `NURSEUS.EXE` runs; read-only game
  directory → elevation or the documented manual steps; idempotent re-install;
  never overwrite an existing user-supplied manifest; **precondition check**
  (NF-4): the target EXE must really import `fakecdda.dll` (the import is a
  bolt-on — descriptors 12/13 name it from inside `.imports`, raw end == EOF;
  verified with pefile) and the stock shim must exist to rename, else refuse
  with a clear message.
* Uninstall reverses all of it. INI: next to the game when writable, else
  `%APPDATA%\PnModern\config.ini` (read-only install dirs).
* A dev-only loader that injects the DLL into a pristine staging copy was planned but
  never built. Staging runs use the proxy instead, and nothing extra ships.

## 2. Loader: proxy `fakecdda.dll`

* The EXE imports `fakecdda.dll` (`mciSendCommandA`, `mixerSetControlDetails`)
  at load time as a hard requirement (`F04`, `E01`); it exports 3 named
  functions; `NURSEUS.EXE` is its only consumer. This is the one clean
  app-directory load point.
* OBSERVED (session 10; kept log `work/tmp/modules_probe_run.txt`, 22:04 —
  artifacts are overwritten per run, cite mtime): with the user's five custom
  DXVK DLLs staged, a 12 s run loaded **64 modules, none of
  `d3d8/d3d9/d3d10core/d3d11/dxgi`** (`PYTHONIOENCODING=utf-8 python
  tools/m0_run.py 12 > work/tmp/modules_probe_run.txt`; probe in
  `tools/m0_run.py`, 1 Hz sampling — sub-second loads invisible). The game also
  runs without them (`m0_observed.md`). Static form (`E01`/`E02`/`E16`): none of
  the five names is an import or a string in the image. App-directory entry
  points that do exist: `fakecdda.dll` / `fakemm.dll` (`F04`) — "no drop-in
  filename" means the five DXVK names, not those two.
* Forwarding: lazily `LoadLibraryW("fakecdda_real.dll")` on first call; if it
  is missing, log and fail the call (review Y3: the WINMM fallback is not
  behaviour-preserving — real CD-DA instead of the MP3 shim); never load inside
  `DllMain`. The three exports are emitted via a **`.def` file as the primary
  mechanism** (Y1 — `#[export_name]` alone may still stdcall-decorate on i686)
  and verified undecorated with `tools/pe_report.py` after build.

## 3. Presenter (a): GDI in place

### 3.0 Virtual desktop (user decision, session 14 — supersedes the window/geometry rules below)

**Why.** Up to session 14 the presenter was *allow-by-default*: the engine's window and
display calls went through unless a hook had been written for a case already seen to go
wrong, and the engine lived in a *mixed* coordinate world (the game window's client in
logical units, the screen, mouse messages, `GetWindowRect` and other windows in physical
ones). The user's in-game right-click menu test (session 14) hit four bugs from that one
cause, each found only by playing: the popup menu opened off screen (`WM_RBUTTONUP` carries
physical client coordinates, the engine ran them through the logical `ClientToScreen` →
`TrackPopupMenu`, call site `0x41fb1d`); the engine's *Window mode* command (`0x41c4d0`)
re-placed the text box from physical `GetWindowRect` values and drew an oversized copy
of it; it also called `SystemParametersInfoA(SPI_SETWORKAREA, NULL)` (`0x41c70a`), which left
the user's desktop work area at 640×480 (OBSERVED, run `mn1`: `workarea=(0,0,640,480)`),
so other windows maximized tiny; and the user saw dialogs open off the top of the
screen. The user rejected fixing these one by one and chose this model.

**Model.** The engine runs on a **virtual 640×480 monitor (V)** that it owns completely. It
is in its original fullscreen setup, the one the game was written and tested for:

* its game window is a borderless 640×480 window at V (0,0), so V screen = game-window
  client = frame logical space; the text box and every other engine window have a V rect;
* **every** geometry answer for the engine's windows, the desktop window, the screen
  metrics, the work area, the current display mode and the cursor is given in V
  (`GetWindowRect`, `GetClientRect`, `GetWindowPlacement`, `Client/ScreenToClient`,
  `MapWindowPoints`, `GetSystemMetrics` screen/virtual-screen sizes,
  `SPI_GETWORKAREA`, `EnumDisplaySettingsA(current)`, `GetCursorPos`, `GetMessagePos`,
  client-coordinate mouse messages, `WM_SIZE`), so the engine's screen and client
  spaces always agree;
* **every** request on its own windows is *recorded* in V and never applied to a real
  window (`SetWindowPos`/`MoveWindow` geometry and z-order, `SetWindowLongA` style and
  exstyle); only show/hide reaches the real windows;
* **deny by default** for global side effects: `ChangeDisplaySettingsA`, every
  `SystemParametersInfoA` action except `SPI_GETWORKAREA` (static scan: the only two call
  sites are `0x41c70a` SET and `0x435cb4` GET), `ClipCursor`, and any
  `SetWindowPos`/`MoveWindow`/`ShowWindow` on a window of another process (the taskbar).
  Each refusal reports success to the engine and is logged;
* one transform **V → physical** (`frame origin on screen + v × scale`) is applied only at
  the real boundaries: `TrackPopupMenu`, `SetCursorPos`, the cursor reads, and the engine's
  dialogs (real windows: an engine move of one is replaced by "centre over the game
  frame, clamped to the monitor's work area"). The cursor is reported in V unless one
  of the engine's dialogs or menus is the active window: a modal dialog runs MFC code
  in real coordinates while the game's own loop is blocked.

**The real window is the presenter's**, not the engine's: windowed (decorated, centred
on the work area at first show, user-movable, **never topmost**) or borderless fullscreen
(the monitor rect, not topmost). The engine's *Window mode* menu command (2001) switches
the presenter between the two at runtime: the plan is re-computed and the frame
re-presented. *Default window position* (32772) re-centres the windowed host. Neither
command reaches the engine. The text box's real window stays alive (for the engine's own
logic), invisible and click-through. Its place in the composition is its V rect, read
live.

**Kept from the earlier design:** the present redirect and `compose`/`sink` (§3, §7), the
config/INI, the DPI decision (A2), the kill switch (`enabled=0`: every hook passes
through). Remaining real-window boundaries are listed in `crates/pn_presenter/src/virt.rs`
(the module doc). A new boundary is added there, not as a one-off hook.

**Verified (session 14, staging copy, `tools/play.py`).**
OBSERVED: the right-click menu opens at the cursor (vd3: menu rect origin (959,557) =
cursor (959,557)); Properties/LOAD/SAVE open centred over the game frame, title bars
on screen; *Window mode* switches windowed ↔ borderless fullscreen and back with the text
box composed inside the frame each time and no second box window visible; the work area
stays `(0,0,1920,1080)` throughout, and the engine's `SetWindowPos` on the taskbar is
refused and logged in every run (`flags=0x93`; the `SPI_SETWORKAREA` branch is **not
exercised by these runs** — see below); dialogue click-advance works
windowed at `scale=fit` (vd3), at the default `scale=auto` (vd5, 2×) and fullscreen
(vd6, 2.25× letterboxed); 0 panics in all runs. The text box window is classified at
creation (`CreateWindowExA`: Afx class + `WS_POPUP`, not `WS_CHILD`) and its V rect
comes from the create arguments (4,376,632,104): read back from the real window at
registration it was empty (vd2). **User-verified:** release zip r3 on the real install
"works" for all four reported bugs (session 14, user report). Not tested: multi-monitor
setups (UNKNOWN).

**What these runs do not show (review s14b, phase 2 — corrected claims).** The work-area
figure and the "menu at the cursor" pair are `tools/play.py` **stdout**, which no vd run
captured; vd7 re-ran the vd3 script with `| tee work/tmp/vd7_play.log` and gives the work
area `(0,0,1920,1080)` at both toggles, the two dialog rects and the two host rects — but
not the menu: a driver started from a background shell cannot take the foreground, and its
new focus guard (review s14b m-4) then skips the right-click, so the menu half of that
claim still rests on vd3's log (`popupmenu v=320,240 -> 959,557` + `#32768 rect=959,557`).
The SAVE dialog did not appear in vd7 either (its 1.5 s window, game not foreground);
vd3 shows three dialogs. No `refused spi` line exists in any vd log: under V the engine's
*Window mode* command never reaches it, so its `SPI_SETWORKAREA` site (`0x41c70a`) is never
executed and the refusal branch is **UNTESTED** — the symptom it guards (run `mn1`, before
V) is real, the guard is code-read only. Same for the `ChangeDisplay` reset and the
`flags=0x53` taskbar call: logged in vd4 only. `WM_WINDOWPOSCHANGING`/`CHANGED` reached the
engine with physical rects in vd5/vd6 and are now answered in V (`virt::route`, vd7: 0
panics, dialogue and dialogs unaffected); the un-virtualised coordinate APIs that remain
are listed in `s14_deferred.md`, with `GetUpdateRect` measured at **0 calls** in vd7.

**Feature-completeness constraint (user directive, session 11 — read this before coding).**
The project **must** handle the game's features correctly, explicitly including the dialogue
text box and the **movie playback path**. Whether that is possible with GDI in place, or
whether the presenter has to escalate to SDL (rung (b)/(c) in §4), is **unproven and must
be decided by testing**, not by assumption. The test comes *after* the plan below is
carried out; until then, **no code may hardcode an assumption about either outcome**. In
particular:

* the present predicate stays "destination DC identity" (`WindowFromDC`), and the source
  side stays generic — a source may be a memory DC with a DIB section, a memory DC with a
  1-bpp DDB, or a **window DC** (`GetDIBits` cannot read a window DC; the spike's
  `NO-CAPTURE` marker exists for exactly that case). **Correction (session 12, verified
  against every kept frame log from the phase A and B runs):** no window-DC source was
  ever observed — **zero** rows have an unreadable source (`bpp=0`, `src_bmp=0x0`), so
  the generic-source rule is a *precaution*, not a measured case;
* nothing may assume the frame is a single 640×480 32-bpp surface, that movies are GDI
  pixels, or that movies are not;
* keep the escalation seam explicit (a small frame-sink trait: GDI implementation now, SDL
  implementation later if the test demands it) instead of wiring GDI assumptions through
  the geometry and window code.

Observed pieces of the movie path (INFERRED, to be tested, `s11_phase_a.md` §3): DirectShow
`FilterGraphWindow`/"ActiveMovie Window" instances (320×240, `visible=0`, created in
sequence at (26,26) → (52,52) → (78,78) → (104,104)), VM waits that stall for 90 s+ on
them (NF-13), and offscreen 544×320 blits (exactly a video frame size). **Correction
(session 12, before Phase B's movie test):** the 544×320 blits are *not* a window-DC
copy-out — their sources read back as ordinary 1-bpp and 32-bpp 544×320 bitmaps, and they
go to **memory** DCs (`grid2/frames.csv:151536`, `grid3/frames.csv:94235`,
`grid4/frames.csv:90621`; `dest_win=0x0`). There is currently **no evidence of any movie
pixel reaching a readable GDI surface**, so the test below starts from "the movie is
invisible (window `visible=0`) and the engine stops presenting during that phase" rather
than from a suspected copy-out. If movie pixels *are* found in a GDI surface (the window
DC, or the engine's frame DIB), GDI in place can present them (stretch-blit from that
source); if the renderer uses an overlay or a backbuffer with no GDI pixels, it cannot,
and that is the escalation trigger.

**Movie-path verdict (session 13, supersedes the two paragraphs above; downgraded by the
s12/s13 review):** INFERRED, not established —
the traced paths and the install folder show **no movie content to present**. The engine has an AVI player (op `0xF3 AVIP`, `E18`; it
opens `VideoDir + name + ".AVI"`, `E19`), but (1) no AVI file ships; (2) across all 803
ISF entries there are 16 AVIP-shaped byte runs and **0** plausible file names (`E19`,
asserted by `unpack.py --selftest`); (3) op `0xF3` was fetched **0** times in every traced
run (~30 census lines in `work/tmp/*.log`, including the full intro path); (4) the
"ActiveMovie Window" is the stock `fakecdda.dll` playing `TKxx.MP3` through
`open … type mpegvideo` (`E19`): it appears ~80 ms after the MCI open calls and is
destroyed right after the MCI close .
The "VM waits that stall" were op `0x85 IG` — *in-screen mouse input* (`E18`), i.e. the
engine waiting for a click — and op `0x47 IF`, not a video wait. INFERRED: `AVIP` is unused
engine capability (ScriptMagic is shared across titles). **Review caveats:** leg (2) has no
positive control (string operands of the other name-carrying ops are empty in this release
too, so "0 plausible names" cannot fail); the one tiled `0xF3` (`DAY11_7.ISF @0x03a8`) was
never reconciled; leg (3) covers only the opening and Continue-save scenes of 803 entries;
leg (1) is this install folder only (`VideoDir` is configurable). The earlier "VM waits that
stall 90 s+" (NF-13, Phase A "video freeze") were also our own census deadlock (`WINS` held
across `GetWindowTextW`, present since `a41b781`; `grid3/4/5` end on a new-window line), not
a measured engine video wait. User confirmation **A4** is still open. The sink seam stays, so a build
that does ship AVIs would be a test, not a rewrite.

Presentation surface (triage-revised, session 11):
the 640×480 frame is presented by a `BitBlt` into the game's own `ScriptMagic` window DC
(12–25 blits/s during the observed phases, OBSERVED). During dialogue the engine **also**
creates a second top-level window for the text box (`Afx:…`, 632×104 at (4,376),
`WS_POPUP|WS_VISIBLE|WS_CLIPSIBLINGS`, `WS_EX_TOPMOST`) and draws the box background plus
per-glyph bitmaps into *that* window's DC (NF-12) — so "no second window" holds only
outside dialogue. All window-DC blits (scene and text box) read 32-bpp sources and are
SRCCOPY, so scaling the text box is a plain SRCCOPY scale. Also OBSERVED (NF-17): the
engine composites the same frame into offscreen memory DCs (85×640×480, 3×544×320, one
1440×720, some of them 1-bpp), so the present predicate must be *destination DC identity*,
never blit size. Never sample the engine's window with `PrintWindow` — it blocks forever
while the engine is in its non-pumping poll wait (NF-18); read via `BitBlt`.
How the text box follows the configured scale is the §7 decision. Escalation ladder
stays in §4.

* Hook installation (R1, triage-revised): whether the loader snaps the IAT
  before or after static `DllMain`s is **unproven** (the s9 "pre-snap" reading
  was taken at `CREATE_SUSPENDED`, before any loader code ran; it shows file
  bytes, not an ordering). The fix is order-agnostic (NF-2): `DllMain` reads
  the host's import directory; one waiter thread patches each slot as soon as
  its value **points outside the host image** (a snapped address). The recorded
  import RVA is *not* the test — if the snap already happened it never changes
  and a change-based wait silently no-ops into fail-open. Bounded (~5 s),
  logged, fail-open; re-verify after patch. `DllMain`: memory reads +
  `CreateThread` only (Y2).
* Hooks (fixed VAs are legitimate: `E02` §DllCharacteristics `0x0000`, `E01`
  chars; patch from the host's own import directory — no hardcoded VAs in the
  release; the tracer's fixed VAs were session instrumentation). Guards: skip a
  slot already inside our module (double-load); never call a hooked function
  from a hook body (`GetDIBits`/`StretchDIBits` are not host imports, so they
  cannot re-enter — O2); every hook body `catch_unwind` (AGENTS); the original
  pointer is read from the slot **after** snap detection, sanity-checked to lie
  outside the host image, else resolved with `GetProcAddress`.
  * `ChangeDisplaySettingsA/W` → suppress, return success. OBSERVED: the game
    mode-switches the desktop to 640×480 (`m0_observed.md`); any blur on an LCD
    is therefore the panel/GPU scaler, which this suppression removes.
  * Present path: install observers/hooks on **both `BitBlt` and `StretchBlt`**
    (NF-5 — run11 kept data: three hot 640×480 BitBlt dests 3549/256/255, plus
    16 640×480 StretchBlt calls with dests never classified,
    `work/tmp/grid_run11.log:92`). OBSERVED: text is composited as bitmaps into
    the frame (`s8_handoff.md` §5); the single-window claim is **INFERRED**
    there (WindowFromDC never logged) and is decided by the M0 census.
    Transform **every** blit whose dest is the game's window DC — full-screen
    *and* partial, or overlays land 1:1 (O1); classify with `WindowFromDC`.
  * Input world model (R2 — confirmed, and already load-bearing): the engine's
    wait loop *observes* `GetCursorPos` → `ScreenToClient` → `PtInRect`
    (`s7_handoff.md`); imports add `GetMessagePos`, `SetCursorPos`,
    `ClientToScreen`, `MapWindowPoints`, `GetClientRect`, `ClipCursor`,
    `SetCapture`, `IsIconic` (`E02` §USER32), and the byte scan finds 9
    `GetCursorPos` / 12 `ScreenToClient` call sites. Message-lParam rewriting
    alone is insufficient. One geometry module maps them with explicit **space
    rules** (NF-8): `GetCursorPos`/`GetMessagePos` stay physical-screen;
    `ScreenToClient`/`ClientToScreen`/`GetClientRect`/`MapWindowPoints` are
    virtualised (logical client = 640×480 at 0,0); `ClipCursor` scales up from
    logical; `WM_MOUSEWHEEL` screen coords map like cursor position. M1 logs
    API pairs/spaces actually used.
  * Window subclass (`SetWindowLongA`) on the game window **and on the dialogue text-box
    window** (NF-12): sizing policy, topmost policy, normalising the engine's own window
    calls (`WM_WINDOWPOSCHANGING`; the engine imports `SetWindowPos`, `MoveWindow`,
    `ShowWindow`, `GetWindowPlacement` — `E02`, review O5), (no `WM_PAINT` handler: the engine re-presents ~12-25/s; review s12/s13 P1-6)
    `WM_ERASEBKGND` so resizes/exposes do not flash (review O6).
  * Alt-tab (O4 confirmed: arithmetic + `m0_spy.py` reads `GWL_STYLE`): defocus
    parks the window at `(-32000,…)` with style `0xb6000000` =
    `0x96000000 | WS_MINIMIZE` — self-minimize INFERRED. M1 logs the
    **arguments** of the engine's own `ShowWindow`/`SetWindowPos` calls (NF-9:
    counts and the message sequence cannot reveal a `ShowWindow(SW_MINIMIZE)`);
    the fix goes in `WM_ACTIVATE`/`WM_SYSCOMMAND` once the mechanism is
    identified.
* Presentation: read the source back as 32-bit `GetDIBits` and draw with
  `StretchDIBits` + explicit `BITMAPINFO` — chosen for the scaling/filter path
  and for dump-vs-screen comparison, **not** for palette correctness (R4
  refuted in triage: no palette API is imported, so a realised-palette mismatch
  is unreachable; both paths read the same colour table). Wrap the draw in
  `SaveDC`/`SetMapMode(MM_TEXT)`, preserve the DC's clip region, black
  letterbox fill; `SetStretchBltMode` per filter (HALFTONE + `SetBrushOrgEx`).
* DPI (O3, partially refuted in triage): the mechanism is not settled offline.
  The m0hold manifest's `<dpiAwareness>PerMonitorV1</dpiAwareness>` is not a
  documented enum value (System / PerMonitor / PerMonitorV2) and is a candidate
  cause of the SxS failure — retest with a valid `PerMonitorV2` and/or
  `SetProcessDpiAwarenessContext` at load. 150 % stays an acceptance case.
  **Measured (session 12, dev box):** the 150 % case is the *current* state —
  `GetSystemMetrics` = 1280×720 (virtual, `GetDpiForSystem` 96) while
  `GetDeviceCaps(DESKTOPHORZRES)` = 1920×1080 physical, i.e. an unaware game is
  DWM-upscaled 1.5×. The presenter correctly sizes in the *virtual* space an unaware
  process lives in (a virtual-sized window is what covers the screen), so the frame is
  drawn at the `fit` scale (921×691 on this box) and then upscaled by DWM; the crisp path
  is the dpiAware manifest (release item), after which the same layout code computes a
  native 2.17× scale. This is why the manifest is part of the M3 deliverable, not a
  nicety (`geom::physical_desktop` logs both sizes on every run).
  **Measured (session 13, staging copy):** the external manifest was *never in effect*
  in earlier runs — Windows caches the "no external manifest" result per EXE until the
  EXE's timestamp changes; after `touch NURSEUS.EXE` the minimal `dpiAware=true` manifest
  gives `SYSTEM_AWARE` (`GetProcessDpiAwareness`, read from outside), `PerMonitorV2`
  gives `PER_MONITOR_AWARE`. The m0hold SxS 14001 is caused by a malformed Windows-8
  `supportedOS` GUID (`…d69d4a6a6e38d}`, one hex digit too many): with only that fixed it
  launches, but `PerMonitorV1` is not a valid value and the process stays UNAWARE.
  DPI-aware runs plan `scale=2.1667 client=1387x1040` (windowed) and `2.25, 1920x1080,
  origin 240,0` (fullscreen) and render natively (no DWM upscale). The staging copy is
  left DPI-unaware (variants parked as `NURSEUS.EXE.manifest.dpiaware` / `.pmv2`) so the
  documented recipes keep their coordinates. Manifest vs. `SetProcessDpiAwarenessContext`
  from the DLL is an M3 decision.
  **Measured (session 14, staging copy, no manifest):** the DLL route works — INI
  `dpi_aware=1` makes `DllMain` call `SetProcessDpiAwarenessContext(SYSTEM_AWARE)`
  (log `dpi system-aware ok=1 err=0`); `GetProcessDpiAwareness` read from outside = 1;
  the plan shows `desk=1920x1080 scale=2.166667 client=1387x1040`; `play.py` with
  `PLAY_AWARE=1` reaches the dialogue box natively, 0 panics (runs pb32/pb33,
  OBSERVED). Default `dpi_aware=0` is unchanged (pb34: awareness 0, `desk=1280x720`).
  **Decided (user, session 14): the DLL route ships, `dpi_aware` defaults to 1**; no
  manifest, no EXE touch. DPI-aware fullscreen dialogue reached (pb36, letterbox 240,0).
* Engine device queries (NF-3, runtime supersedes the byte scan): the byte scan found  `GetDeviceCaps` ×2 and `GetSystemMetrics` ×1 *direct* call sites, but that is a lower
  bound — runtime shows ≥3 distinct `GetDeviceCaps` indices (12, 88, 90) and ≥9 distinct
  `GetSystemMetrics` indices, and 2 `ChangeDisplaySettingsA` sites hold (mode set, then
  `NULL,0` on defocus). Suppressing the mode switch changes the answers these calls get.
  M1 logs the DEVMODE the engine requests (measured: `fields=0x7c0080`, 640×480, 32 bpp,
  120 Hz) and the query results, **before** the hook inventory is frozen.
* Defaults: windowed = *computed* largest comfortable integer scale for the
  desktop (2× at ≥1080p, 1× at ≤720p), fullscreen = borderless aspect-fit
  smooth, topmost as the game sets it, fullscreen on the window's monitor.
* Failure policy (review O2): install/patch failure or repeated hook exception
  → restore original slots, log, continue unhooked; `enabled=0` does the same
  at load. The shim must never block or crash the game.

## 4. Escalation ladder (agreed)

(a) GDI in place → (b) SDL child window inside the game window (GPU scaling;
same-window focus semantics) → (c) SDL separate window with input forwarding
(only for a genuine separate-window requirement — it is the *worst* place for
the text-box risk, not the escape hatch).

Escalate only on: a measured scaled-present cost > ~15 ms/frame at the highest
target resolution on a PC-class machine; or a reproduced correctness blocker
that (b) demonstrably fixes; or an explicit decision to want GPU-only filters.
None are currently met. Measurement (review Y4): QPC around the present path,
logged per run; trigger on p99 > 15 ms at the target resolution, not a single
outlier.

## 5. Config EXE and settings

Rust + egui, one EXE. Settings (initial iteration, tentative by agreement):
mode (windowed / borderless fullscreen); scale 1×/2×/3×/4×/fit; filter
(auto / nearest / smooth); Alt+Enter fullscreen toggle; `enabled=0` kill
switch. No live apply otherwise.

## 6. Milestones and acceptance

* **L2-M0 — instrumentation facts** (tracer + module probe; facts must come
  from runs, not inference):
  1. present dest identity: `WindowFromDC` on 640×480 blit dests (window vs
     memory DC);
  2. window model across phases: enumerate the process's top-level windows
     during title → menu → dialogue → options/save (closes U4/F15 scope for
     L2);
  3. frame pixels: fresh run — source-DIB **and** window-DC dumps as a time
     series, then classify (NF-1: the old "all captures uniform/black" premise
     is stale; the current `work/run/m0_*.png` are session-10 probe frames
     (mtime 21:27) showing black → grey → white+logo — the AngelSmile logo is
     readable at t≈6 s, engine renders). If the *fresh* measurement shows
     engine-side black frames with a healthy screen path: stop and investigate
     before L2-M2;
  4. module baseline — DONE (64 modules, kept log
     `work/tmp/modules_probe_run.txt`; §2).
* **L2-M1 — proxy spike**: DLL loads as `fakecdda.dll`, forwards audio, installs
  hooks, logs frames/`WindowFromDC`/window census; game reaches dialogue with
  the spike loaded in a tracer grid run. **Delivered (session 11, `s11_phase_a.md`;
  re-measured on the reviewed build after the Phase A review)**: 3 undecorated exports
  (`pe_report.py`), 20/20 hooks installed with the order-agnostic snap test, dialogue
  reached, present identity = window DC of `ScriptMagic` + the text-box window's partial
  blits (all SRCCOPY), input poll counts (`GetCursorPos`/`ScreenToClient` ~10 kHz,
  `IsIconic` 37 M), `ShowWindow`/`SetWindowPos` **argument** log, DEVMODE + device
  queries, read-back cost (upper bound p50 ~10 ms), and a fresh frame time series
  (1753 unique window-DC frames; vision pass reads menus and dialogue text). Acceptance: undecorated exports via
  `pe_report.py` (undecorated, `.def` primary); hooks install and fire before
  the mode switch (timing measured — the "≈1 s" figure has no artifact, NF-10);
  present identity per phase: identify the window DC and **expect several
  640×480 dests plus partials** (run11: 3549/256/255 + 632×104/211×50,
  `work/tmp/grid_run11.log:92`; the s8 §5 dialogue shape is single-run and
  unkept — a disagreement is data, not failure); input-path API/space log
  (R2/NF-8); `ShowWindow`/`SetWindowPos` **argument** log (NF-9); DEVMODE +
  device-query log (NF-3); engine-vs-screen pixel comparison with time series
  (NF-1); audio path exercised without failure.
* **L2-M2 — presenter core**: mode suppression, window sizing (resizable
  windowed with OS decoration; borderless fullscreen), scaled present with
  filters, **the dialogue text box scaled and repositioned with the frame**
  (`04` §7 decision), alt-tab fix, mouse remap. Acceptance: dialogue reached and visible
  on screen at the configured scale **including the text box**, alt-tab returns the game on-screen;
  clicks land at the right 640×480 coordinates — plus, from the session-11 directive (§3,
  feature completeness): **the movie path tested end to end** (a run that reaches the
  DirectShow video phase; recorded verdict *GDI works* or *escalate per §4*, with the
  evidence in the run log and, if it fails, the escalation trigger named).
  **Status (session 13): met in the merged-window form (§7)** — dialogue rendered through
  the transform with the text box in place (`work/tmp/pb21_sheet.png`, `pb26_sheet.png`),
  alt-tab (pb28), clicks advance the dialogue, move/minimise/restore keep one window
  (pb26); movie verdict in §3 (INFERRED: no movie content on the traced paths; the "movie path tested
end to end" clause is **not** met, pending A4). Resizable
  windowed mode is the
  one tentative item (review Y5): if the engine's window fights exceed a
  bounded effort, it demotes to v1.1 and fixed presets ship.
* **L2-M3 — config + install + release**: egui EXE, install/uninstall, INI
  policy, zip, CI, README/LICENSE.
  **Status (session 14): built, partly verified.** `crates/pn_shared` (INI model shared by
  DLL and EXE; round-trip and README-example tests), `crates/pn_config` → `PN-Patch.exe`
  (eframe/glow, **x86_64 only**: a 32-bit manifest-less EXE is UAC-virtualized and would
  install into VirtualStore; `--install|--uninstall [--game DIR]` headless, used for the
  UAC re-launch), DLL log rotation at 1 MiB (`pn_modern.log.1`), root `README.md`,
  `LICENSE-MIT`, `tools/release.py` (builds, tests, asserts PE machines,
  the three undecorated exports and the marker, writes `target/dist/pn-modern-<v>.zip`).
  OBSERVED: headless install → reinstall → uninstall on real files restores the stock
  shim byte-identically; wrong folder and running game refused (exit 1); PN-Patch
  uninstall + install from the zip on `work/run`, then a normal (non-spike) play run with
  defaults reached dialogue; log rotated. **User-verified (session 14, user report):** the
  release zip unpacked into the real install under `C:\Program Files (x86)\…`, PN-Patch
  Install through UAC, Play: the game ran (the r1 zip, which surfaced the right-click menu
  bugs that led to §3.0), and the r3 zip with the virtual desktop "works", including the
  right-click menu, Window mode, Properties/Save/Load and other windows after the game
  closed. Not checked separately: the GUI's Save button by click.
  **Cut from the spec:** CI (no git remote to run it on), the DXVK `_backup/` offer
  (the five DLLs are inert, `E16`), the dev loader (never built: staging uses the proxy), an
  Alt+Enter toggle (needs a DLL hotkey; v1.1 if wanted).
* **L2-M4 — measurements + regression**: per-frame present cost at
  1080p/4K (escalation triggers) — **measured (session 13, release build, observers off,
  `PN_COST=1`):** p99 per present ≤5.6 ms (fit 1.44×), ≤5.9 ms (fullscreen 1.5×),
  ≤8.9 ms (DPI-aware 2.17× windowed), ≤11.3 ms (DPI-aware fullscreen, 1440×1080 frame);
  `tools/gdi_bench.py` puts one full-frame HALFTONE stretch at ~19 ms (1440p) and ~37 ms
  (4K), COLORONCOLOR at ≤4 ms even at 4K — the trigger fires for *smooth* scaling above
  1080p. **Decided (user, session 13, A1): GDI for v1**; `filter=auto` is nearest at
  integer scales, smooth up to a 1080p-class frame (≤1 600 000 destination pixels, which
  includes 1440×1080), nearest above; an explicit `filter=` wins; a GPU sink only if
  needed later; visual regression harness (`shot.grab()`
  during tracer runs; frame-hash/pixel-diff vs `decode_ggd` renders).

## 7. Out of scope for v1

**Superseded (session 13, user requirement): the text box is merged into the game window.**
Two top-level windows drift apart when the game window is moved or minimised, so the box
must not be a separate on-screen window unless every merged design is unreasonable. The
merge is `compose.rs`: the engine's blits into its game window and into its box window go
to presenter-owned logical layers (frame 640×480, box), composed (frame, then the visible
box) and presented to the game window only. The engine's box window stays alive for the
engine's logic but is invisible (layered, alpha 0), click-through, never activated,
without a taskbar entry, and keeps the engine's own **unscaled** 632×104 geometry
(scaling it leaked physical units back into the engine — the box grew on every move,
pb22). `WM_MOVE` of the game window is swallowed unless the engine moved it itself (its
handler re-places the box and rebuilds the box background from the wrong frame area,
pb23–pb25; the original game was fullscreen and never moved). The session-11 decision
below is kept for the record.

**Decided (session 11, from Phase A fact NF-12):** the dialogue text box is a separate
top-level window (632×104 at (4,376), `WS_POPUP|WS_EX_TOPMOST`), so "no second window"
is false during dialogue. **v1 scales the box with the frame** (user decision,
2026-10-18): Phase B transforms the box window's geometry and its blits with the same
scale/filter as the scene, using the engine's own mask/paint ROPs. Consequences:

* the box window must be subclassed and its geometry normalised exactly like the main
  window (§3) — a presenter that only handles the main window leaves the box at 1:1 on
  top of a scaled frame;
* glyph edges at 3–4× are blocky with `nearest` and soft with the smooth filter; that
  is accepted for v1;
* the two rejected alternatives stay on record: keep the box 1:1 and reposition it
  (crisp but visually inconsistent), or hide the box and re-render text crisply from
  the decoded corpus (the milestone below, still parked).

Crisp re-rendered text (text-box hide/replace milestone later, on top of the
M2 hooks: `WSH`/`WSS` interception + `hide=fade|instant|persistent`);
WinHelp; registration/path shims; exclusive fullscreen; GPU filters;
U3/U5/U6/U7 unchanged.

## 8. Release engineering

* Workspace: `crates/pn_presenter` (cdylib), `crates/pn_config` (egui bin),
  `crates/pn_shared` (INI/path helper for the DLL
  and the config EXE — Y6). Python RE tools stay in `tools/`.
* Our code: MIT. CI: windows-latest, i686 build + the
  `fmt`/`clippy` gate from `AGENTS.md`; release zip on tag. Unsigned DLL taking
  over a DLL name: document AV/SmartScreen expectations and the manual install
  path (review O7/Y7). Neither DXVK nor the stock FakeCDDA files are
  redistributed.
* Cleanup stage: `.gitignore` for `target/`; public `README.md` at the root;
  research notes live under `docs/`.
* Platforms: Windows 10/11 x64 hosts running the 32-bit game; ARM64 untested.

## 9. Traps carried forward

* Staging copy only; `original/` stays byte-identical to `E00`.
* Kept tails/logs/PNGs are overwritten per run — cite run + mtime; re-runs beat
  quoted numbers (NF-1: M0's screen captures no longer exist; the current
  `work/run/m0_*.png` are session-10 probe frames).
* IAT slots hold lookup RVAs (`0x56E8E`) in the file until the loader snaps
  them; the *order* versus static `DllMain`s is unproven (R1) — detect snap by
  "value points outside the host image", never by "value changed" (NF-2).
* Enumerating a WOW64 target's 32-bit modules from 64-bit Python needs
  **`K32EnumProcessModulesEx` with `LIST_MODULES_ALL`** — plain
  `EnumProcessModules` returned only 7 wow64 stubs (NF-11).
* The game window parks at `(-32000,-32000)` on defocus and stays hidden;
  reproduced in `m0_observed.md` and the session-10 module-probe run (t≈8 s).
* `work/run` carries the proxy rename (ours as `fakecdda.dll`, stock shim as
  `fakecdda_real.dll`) for L2 work; `original/` stays the `E00` reference.
