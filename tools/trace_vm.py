"""VM fetch tracer (U2 dynamic). Logs every 0x424e30 fetch via a codecave + ring.

Method: launch staging NURSEUS.EXE suspended, VirtualAllocEx a code page +
an 8 MB ring, patch a JMP at the VM loop entry (0x424e30, fixed address, no
ASLR) to a stub that logs (stream_addr, op_byte) per fetch, resume, feed
input like a player, drain the ring, terminate. No debugger, no hooks lib,
full speed; 64-bit Python can do it all (no thread context needed).

Modes: default (full trace) | grid (centre-out clicks + live census +
handler code dumps + dialogue ring-tail dump) | peek (operand/raw dumps +
heap context + scenario-table probe) | keys (key sweep with per-key deltas).
Dialogue = first ED/ONJS/PM fetch (init has none).

Caveat: live code dumps race the VM (a transient site's bytes may be
reused by the time we RPM them); stable sites (spin loop, PM text) match
across runs, transient ones may show stale bytes.

Answers, empirically: which gap bytes fetch as ops (vs dead), whether tag
tails fetch, where prefix-skips land, where JP/JS/ONJS/ONJP transfer to.
"""
import ctypes
import os
import struct
import sys
import time
from ctypes import wintypes
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUN = ROOT / "work" / "run"
EXE = RUN / "NURSEUS.EXE"
OUT = ROOT / "work" / "tmp" / "vm_trace.bin"

sys.path.insert(0, str(ROOT / "tools"))
from m0_run import press_key, windows_of, click  # reuse staging input/window helpers

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
u32 = ctypes.WinDLL("user32", use_last_error=True)

# Title-menu entry centres, measured from dumped frames of the phase A runs
# (640x480, +/-3 px): Start (125,60),
# Continue (123,112), Memories (125,162), Music (125,211), After Diary (125,261),
# Exit (124,311). "Start" goes straight into a scene; the After Diary path
# ((320,195) in its sub-menu) plays the slideshow whose video freeze wedged 3 of 7 runs.
TITLE_START = (125, 60)
# Phase B entry choice: the "Start" (intro) path reached the dialogue state in pb4/pb7/pb9
# but the engine's art load wedged and the screen stayed black (0x85/0x47 loops,
# s12_phase_b.md §5); the "After Diary" path rendered its dialogue in the Phase A run 6.
TITLE_AFTER = (125, 261)
AFTER_DIARY = (125, 261)
TITLE_CONTINUE = (123, 112)
# Which title entry the tracer takes (PN_TRACE_ENTRY=continue|after|start). "Continue"
# loads the staging copy's own save and lands directly in a scene, which is the fastest
# route to a rendered dialogue; "start" is the intro, which wedged black in three Phase B
# runs; "after" is the After Diary sub-menu path.
_ENTRY = os.environ.get("PN_TRACE_ENTRY", "after").lower()
TITLE_ENTRY = {"start": TITLE_START, "after": TITLE_AFTER,
               "continue": TITLE_CONTINUE}.get(_ENTRY, TITLE_AFTER)
AFTER_DIARY_MARIA = (320, 195)


def _env_num(name, default):
    try:
        return float(os.environ.get(name, default))
    except ValueError:
        return float(default)


# Presenter layout (Phase B): the tracer works in the engine's logical 640x480
# coordinates; with the presenter loaded the visible frame sits at
# (origin + p*scale) on the physical screen. Set PN_TRACE_ORIGIN=x,y and
# PN_TRACE_SCALE=s (defaults 0,0 and 1.0 = presenter off/1:1) — both are logged
# in the run log's `plan` and first `present` lines.
_origin = os.environ.get("PN_TRACE_ORIGIN", "0,0").split(",")
LAYOUT_ORIGIN = (int(_origin[0]), int(_origin[1]) if len(_origin) > 1 else 0)
LAYOUT_SCALE = _env_num("PN_TRACE_SCALE", "1")


def phys(p):
    """Logical 640x480 point -> physical screen point for the click helpers."""
    return (round(LAYOUT_ORIGIN[0] + p[0] * LAYOUT_SCALE),
            round(LAYOUT_ORIGIN[1] + p[1] * LAYOUT_SCALE))


def steal_foreground(hwnd):
    """Take the foreground for `hwnd`, reliably.

    Plain SetForegroundWindow silently fails when the calling process is not the
    foreground one (measured: the pb7 defocus step was a no-op — no WM_KILLFOCUS in the
    run log), so attach to the target's input queue first (the classic workaround).
    Returns the foreground HWND after the attempt, for verification.
    """
    target_tid = u32.GetWindowThreadProcessId(hwnd, None)
    my_tid = k32.GetCurrentThreadId()
    attached = False
    try:
        attached = bool(u32.AttachThreadInput(my_tid, target_tid, True))
        u32.ShowWindow(hwnd, 9)  # SW_RESTORE
        u32.SetForegroundWindow(hwnd)
        u32.BringWindowToTop(hwnd)
    except Exception as e:
        print(f"steal_foreground failed: {e}")
    finally:
        if attached:
            u32.AttachThreadInput(my_tid, target_tid, False)
    time.sleep(0.5)
    return u32.GetForegroundWindow()

LOOP = 0x424E30          # VM loop entry (fixed: VC6, no ASLR)
STOLEN = bytes([0x53, 0x56, 0x8B, 0xF1, 0x83, 0x3E, 0x00])  # first 7B at LOOP
BACK = 0x424E37          # resume after stolen bytes
NENT = 1 << 20           # ring entries (8B each)
MASK = NENT - 1
# present-blit hooks (M0 step 3 / U4): GDI32 IAT slots, fixed VAs (VC6, no ASLR)
BLT_FUNCS = {0x44D058: 1, 0x44D0DC: 2}  # StretchBlt, BitBlt -> func ids
BNENT = 1 << 19          # blt ring entries (16B each)
BMASK = BNENT - 1


def build_blt_stub(base, fid, bhead, bring, cell):
    """Hook stub called via patched IAT slot. Logs (fid,hdc,w,h) per call,
    then jumps to the original function (addr kept in [cell]). Args read from
    the caller's stack: [esp+4]=hdcDest, [esp+16]=w, [esp+20]=h (same layout
    for StretchBlt and BitBlt)."""
    out = bytearray()
    out += b"\x60\x9c"                       # pushad; pushfd
    out += b"\x8b\x5c\x24\x28"               # mov ebx,[esp+0x28] (hdcDest)
    out += b"\x8b\x4c\x24\x34"               # mov ecx,[esp+0x34] (w)
    out += b"\x8b\x54\x24\x38"               # mov edx,[esp+0x38] (h)
    out += b"\xa1" + struct.pack("<I", bhead)  # mov eax,[BHEAD]
    out += b"\x25" + struct.pack("<I", BMASK)  # and eax,BMASK
    out += b"\xc1\xe0\x04"                    # shl eax,4
    out += b"\x05" + struct.pack("<I", bring)  # add eax,BRING
    out += b"\xc7\x00" + struct.pack("<I", fid)  # mov [eax],fid
    out += b"\x89\x58\x04"                   # mov [eax+4],ebx (hdc)
    out += b"\x89\x48\x08"                   # mov [eax+8],ecx (w)
    out += b"\x89\x50\x0c"                   # mov [eax+12],edx (h)
    out += b"\xff\x05" + struct.pack("<I", bhead)  # inc [BHEAD]
    out += b"\x9d\x61"                       # popfd; popad
    out += b"\xff\x25" + struct.pack("<I", cell)  # jmp dword ptr [cell] (orig)
    assert len(out) <= 0x80
    return bytes(out)


class STARTUPINFO(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR),
                ("lpDesktop", wintypes.LPWSTR), ("lpTitle", wintypes.LPWSTR),
                ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
                ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD),
                ("dwXCountChars", wintypes.DWORD), ("dwYCountChars", wintypes.DWORD),
                ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD),
                ("lpReserved2", ctypes.c_void_p), ("hStdInput", wintypes.HANDLE),
                ("hStdOutput", wintypes.HANDLE), ("hStdError", wintypes.HANDLE)]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]


def build_stub(code_addr, head_addr, ring_addr):
    """Codecave bytes with absolute addresses patched in. Returns bytes."""
    out = bytearray()
    out += b"\x60\x9c"                       # pushad; pushfd
    out += b"\x8b\x41\x18"                    # mov eax,[ecx+0x18] (stream)
    out += b"\x8b\x15" + struct.pack("<I", head_addr)    # mov edx,[HEAD]
    out += b"\x81\xe2" + struct.pack("<I", MASK)          # and edx,MASK
    out += b"\xc1\xe2\x03"                    # shl edx,3
    out += b"\x81\xc2" + struct.pack("<I", ring_addr)     # add edx,RING
    out += b"\x89\x02"                       # mov [edx],eax (stream)
    out += b"\x0f\xb6\x00"                   # movzx eax,byte [eax] (op)
    out += b"\x89\x42\x04"                   # mov [edx+4],eax (op)
    out += b"\xff\x05" + struct.pack("<I", head_addr)     # inc [HEAD]
    out += b"\x9d\x61"                       # popfd; popad
    out += STOLEN
    back_rel = BACK - (code_addr + len(out) + 5)
    out += b"\xe9" + struct.pack("<i", back_rel)          # jmp BACK
    return bytes(out)


def read_mem(h, addr, n):
    buf = ctypes.create_string_buffer(n)
    got = ctypes.c_size_t()
    if not k32.ReadProcessMemory(h, ctypes.c_void_p(addr), buf, n, ctypes.byref(got)):
        raise ctypes.WinError(ctypes.get_last_error())
    return buf.raw[:got.value]


def write_mem(h, addr, data):
    done = ctypes.c_size_t()
    if not k32.WriteProcessMemory(h, ctypes.c_void_p(addr), data, len(data), ctypes.byref(done)):
        raise ctypes.WinError(ctypes.get_last_error())
    assert done.value == len(data)


def main(secs=40, grid=False, peek=False, keys=False, blt=False, defocus=False):
    assert EXE.exists(), EXE
    OUT.parent.mkdir(parents=True, exist_ok=True)
    # 8x6 grid over the 640x480 mode-switched screen, centre-out (menus live central)
    cells = [(40 + i * 80, 40 + j * 80) for j in range(6) for i in range(8)]
    cells.sort(key=lambda p: (p[0] - 320) ** 2 + (p[1] - 240) ** 2)
    # key sweep: menu navigation + Y/N prompts + digits + F-keys
    VK = dict(UP=0x26, DOWN=0x28, LEFT=0x25, RIGHT=0x27, RET=0x0D, ESC=0x1B,
              TAB=0x09, Y=0x59, N=0x4E, S=0x53, L=0x4C, C=0x43)
    sweep = ["UP", "DOWN", "RET", "UP", "UP", "RET", "DOWN", "DOWN", "RET",
             "ESC", "Y", "N", "S", "L", "C", "TAB", "RET"]

    si = STARTUPINFO()
    si.cb = ctypes.sizeof(si)
    pi = PROCESS_INFORMATION()
    cmd = str(EXE)
    ok = k32.CreateProcessW(cmd, None, None, None, False, 0x00000004, None,
                            str(RUN), ctypes.byref(si), ctypes.byref(pi))
    assert ok, ctypes.WinError(ctypes.get_last_error())
    hp, ht, pid = pi.hProcess, pi.hThread, pi.dwProcessId
    print(f"pid={pid} (suspended)")
    try:
        MEM_COMMIT = 0x1000
        PAGE_XRW = 0x40
        code = k32.VirtualAllocEx(hp, None, 0x1000, MEM_COMMIT, PAGE_XRW)
        ring = k32.VirtualAllocEx(hp, None, NENT * 8, MEM_COMMIT, PAGE_XRW)
        assert code and ring, "alloc failed"
        head = code + 0x800
        print(f"code={code:#x} head={head:#x} ring={ring:#x}")
        stub = build_stub(code, head, ring)
        write_mem(hp, code, stub)
        write_mem(hp, head, struct.pack("<I", 0))
        # patch LOOP: JMP stub + NOP NOP (staging copy; terminated at end)
        rel = code - (LOOP + 5)
        write_mem(hp, LOOP, b"\xe9" + struct.pack("<i", rel) + b"\x90\x90")
        bring = bhead = 0
        bdrained = 0
        blt_recs = bytearray()           # all blt calls kept (rate is low)
        blt_hooked = False               # IAT patched only once imports snap (post-entry)
        if blt:  # rings up front; IAT patched late (see tick: imports snap post-entry)
            bring = k32.VirtualAllocEx(hp, None, BNENT * 16, MEM_COMMIT, PAGE_XRW)
            assert bring, "bring alloc failed"
            bhead = head + 4
            write_mem(hp, bhead, struct.pack("<I", 0))
            iat_init = {}  # suspend-time slot values (= unsnapped lookup RVAs like 0x56E8E)
            for iat in BLT_FUNCS:
                iat_init[iat] = struct.unpack("<I", read_mem(hp, iat, 4))[0]
            print("IAT pre-snap:", " ".join(f"{k:#x}={v:#x}" for k, v in iat_init.items()))
        assert k32.ResumeThread(ht) != 0xFFFFFFFF
        print("resumed, tracing...")

        trace = bytearray()
        drained = 0
        import collections
        census = collections.Counter()   # op -> fetches (no 787MB file needed)
        head_buf = bytearray()          # first 20k fetches always kept
        events = []                      # (label, bytes) windows around new ED/ONJS/PM
        ed_seen = onjs_seen = pm_seen = 0
        ig_peeked = set()                # stream addrs already operand-peeked
        pm_sites = []                    # dialogue PM streams (region->scenario map)
        stc_sites = []                   # STC streams (+6 vs +45 forms)
        ig_stream = None                 # first IG (0x85) stream: heap-dump anchor
        seen = set()                     # distinct streams code-dumped (grid/keys)
        WATCH = {0x00, 0x04, 0x07, 0x08, 0x2B, 0x3A, 0x01, 0x02, 0x56}  # +GL art loads
        fg = u32.GetForegroundWindow()
        t0 = time.time()
        sent_title_keys = False
        fg_done = False
        defocus_done = False       # alt-tab acceptance: drop focus, check, refocus
        title_seen = False              # closed-loop gates: don't click a transitioning menu
        title_t = 0.0                   # (open-loop clicks poisoned 3/3 runs into music/slideshow loop)
        title_clicked = False
        title_click_t = 0.0
        after_seen = False              # AFTER menu art live -> click Maria (322,192)
        menu_clicked = False
        menu_click_t = 0.0
        last_advance = 0.0
        next_cell = 0
        nclicks = 0                      # actual clicks issued (log accounting)
        wedge_last = 0                   # wedge detector: frozen VM thread post-menu
        wedge_t = 0.0
        wedge_saved = False
        last_click = 0.0
        next_key = 0
        last_key = 0.0
        drained_at_key = 0
        try:
            while time.time() - t0 < secs:
                time.sleep(0.2)
                raw = read_mem(hp, head, 4)
                (head_now,) = struct.unpack("<I", raw)
                if head_now > drained:
                    if head_now - drained > NENT:
                        print(f"OVERRUN head={head_now} drained={drained}; skipping to newest")
                        drained = head_now - NENT
                    # read in chunks (wrap-aware)
                    while drained < head_now:
                        idx = drained & MASK
                        n = min(head_now - drained, (NENT - idx))
                        chunk = read_mem(hp, ring + idx * 8, n * 8)
                        if grid or keys:
                            # single pass: census + live code dump of newly seen
                            # jump/dialogue/load sites (handler discovery)
                            for i in range(0, len(chunk), 8):
                                opb = chunk[i + 4]
                                census[opb] += 1
                                if opb == 0x00:
                                    ed_seen += 1
                                elif opb == 0x08:
                                    onjs_seen += 1
                                elif opb == 0x2B:
                                    pm_seen += 1
                                if opb in WATCH and len(seen) < 4096:
                                    (sa,) = struct.unpack_from("<I", chunk, i)
                                    if sa not in seen:
                                        seen.add(sa)
                                        try:
                                            cb = read_mem(hp, sa, 32)
                                            print(f"new op{opb:02X}@{sa:#x} code={cb.hex(' ')}")
                                            if opb in (0x2B, 0x3A):  # heap-layout specimens: PM fusion + STC+45
                                                fb = read_mem(hp, sa, 256)
                                                (ROOT / "work" / "tmp" / f"heap_{opb:02X}_{sa:#x}.bin").write_bytes(fb)
                                            if opb == 0x56:  # GL stems name the screen; gate clicks on them
                                                if b"after" in cb and not after_seen:
                                                    after_seen = True
                                                    print(f"AFTER menu live t={time.time()-t0:.0f}s")
                                                if b"title" in cb and not title_seen:
                                                    title_seen = True
                                                    title_t = time.time() - t0
                                        except Exception as e:
                                            print(f"code peek failed: {e}")
                            if ed_seen + onjs_seen + pm_seen and drained > 100 and not events:
                                events.append((f"t={time.time()-t0:.0f}s", bytes(chunk[:16384])))
                                print(f"DIALOGUE? t={time.time()-t0:.0f}s ED={ed_seen} ONJS={onjs_seen} PM={pm_seen}")
                            if len(head_buf) < 160000:
                                head_buf += chunk[:160000 - len(head_buf)]
                        else:
                            census.update(chunk[i + 4] for i in range(0, len(chunk), 8))
                            new_ed = sum(1 for i in range(4, len(chunk), 8) if chunk[i] == 0x00)
                            new_onjs = sum(1 for i in range(4, len(chunk), 8) if chunk[i] == 0x08)
                            new_pm = sum(1 for i in range(4, len(chunk), 8) if chunk[i] == 0x2B)
                            if len(head_buf) < 160000:
                                head_buf += chunk[:160000 - len(head_buf)]
                            if new_ed + new_onjs + new_pm and drained > 100:
                                events.append((f"t={time.time()-t0:.0f}s", bytes(chunk[:16384])))
                                print(f"DIALOGUE? t={time.time()-t0:.0f}s +ED={new_ed} +ONJS={new_onjs} +PM={new_pm}")
                            ed_seen += new_ed
                            onjs_seen += new_onjs
                            pm_seen += new_pm
                        # peek wait/jump/load operands once per stream (peek mode only now).
                        # Raw bytes printed; decode is prefix-dependent (1B, or 2B if hi-bit set).
                        # LS/LSBS(0x01/0x02): NUL-terminated scenario name at +prefixlen.
                        if peek:
                            want = {0x47, 0x85, 0x04, 0x07, 0x01, 0x02, 0x2B, 0x3A}
                            for i in range(0, min(len(chunk), 8000), 8):
                                opb = chunk[i + 4]
                                if opb in want:
                                    (sa,) = struct.unpack_from("<I", chunk, i)
                                    if sa not in ig_peeked and len(ig_peeked) < 40:
                                        ig_peeked.add(sa)
                                        if opb == 0x2B and len(pm_sites) < 8:
                                            pm_sites.append(sa)
                                        if opb == 0x3A and len(stc_sites) < 8:
                                            stc_sites.append(sa)
                                        if opb == 0x85 and ig_stream is None:
                                            ig_stream = sa
                                        try:
                                            pb = read_mem(hp, sa, 16)
                                            if opb in (0x01, 0x02):
                                                off = 3 if pb[1] & 0x80 else 2
                                                print(f"op{opb:02X}@{sa:#x} str={pb[off:off+14]!r}")
                                            else:
                                                print(f"op{opb:02X}@{sa:#x} raw={pb[:12].hex(' ')}")
                                        except Exception as e:
                                            print(f"peek failed: {e}")
                            # keep trace small in grid/peek mode; counts carry the signal
                        if not (grid or peek):
                            trace += chunk
                        drained += n
                if blt:  # drain blt ring (wrap-aware, same overrun policy)
                    (bhead_now,) = struct.unpack("<I", read_mem(hp, bhead, 4))
                    if bhead_now > bdrained:
                        if bhead_now - bdrained > BNENT:
                            print(f"BLT OVERRUN head={bhead_now} drained={bdrained}")
                            bdrained = bhead_now - BNENT
                        while bdrained < bhead_now:
                            idx = bdrained & BMASK
                            m = min(bhead_now - bdrained, BNENT - idx)
                            blt_recs += read_mem(hp, bring + idx * 16, m * 16)
                            bdrained += m
                el = time.time() - t0
                if blt and not blt_hooked and title_seen:
                    # Hook once the loader snaps the slots (live != suspend-time value).
                    # Slots already pointing into our code page are skipped (never
                    # chain a stub into itself after a partial tick).
                    try:
                        off = 0x400
                        for iat, fid in BLT_FUNCS.items():
                            cell = code + off + 0x80
                            orig = struct.unpack("<I", read_mem(hp, iat, 4))[0]
                            print(f"IAT {iat:#x} live={orig:#x}")
                            if code <= orig < code + 0x1000:
                                print("  already ours; skipping")
                                off += 0x100
                                continue
                            if orig == iat_init.get(iat) or orig < 0x10000:
                                print("  not snapped yet; retry next tick")
                                break
                            write_mem(hp, cell, struct.pack("<I", orig))
                            write_mem(hp, code + off, build_blt_stub(code + off, fid, bhead, bring, cell))
                            write_mem(hp, iat, struct.pack("<I", code + off))
                            print(f"  hooked fid={fid} stub={code + off:#x}")
                            off += 0x100
                        else:
                            blt_hooked = True
                    except Exception as e:
                        print(f"hook install failed: {e}")
                if defocus and not defocus_done and el > 10.0:
                    defocus_done = True
                    mine = [w for w in windows_of(pid) if w["visible"]]
                    game = mine[0]["hwnd"] if mine else 0
                    # 1. hand the foreground to someone else and prove it left the game
                    other = steal_foreground(fg if fg else 0)
                    away = [w for w in windows_of(pid) if w["visible"]]
                    left = other != game and all(w["hwnd"] != other for w in away)
                    print(f"DEFOCUS left_game={left} fg={other:#x} game_visible={len(away)} "
                          + "; ".join(f"{w['rect']} style={w['style']:#010x}" for w in away[:2]))
                    time.sleep(1.0)
                    # 2. take it back: the window must still be where it was
                    back_fg = steal_foreground(game) if game else 0
                    back = [w for w in windows_of(pid) if w["visible"]]
                    returned = any(w["hwnd"] == game for w in back)
                    print(f"REFOCUS returned={returned} fg={back_fg:#x} game_visible={len(back)} "
                          + "; ".join(f"{w['rect']} style={w['style']:#010x}" for w in back[:2]))
                if el >= 3 and not sent_title_keys:
                    mine = [w for w in windows_of(pid) if w["visible"]]
                    if mine:
                        u32.SetForegroundWindow(mine[0]["hwnd"])
                        time.sleep(0.3)
                    for vk in (0x0D, 0x20, 0x0D):
                        press_key(vk)
                        time.sleep(0.4)
                    print("sent Return/Space/Return")
                    sent_title_keys = True
                    last_advance = el
                elif el > 6 and el - last_advance >= 3.0 and not keys:
                    press_key(0x20)  # advance dialogue steadily
                    time.sleep(0.2)
                    last_advance = el
                if keys and sent_title_keys and el > 8 and el - last_key >= 3.0 and next_key < len(sweep):
                    if not fg_done:
                        mine = [w for w in windows_of(pid) if w["visible"]]
                        if mine:
                            u32.SetForegroundWindow(mine[0]["hwnd"])
                            time.sleep(0.3)
                        fg_done = True
                    print(f"key {sweep[next_key]}: +{drained - drained_at_key} fetches since last "
                          f"(ED={ed_seen} ONJS={onjs_seen} PM={pm_seen})")
                    drained_at_key = drained
                    press_key(VK[sweep[next_key]])
                    time.sleep(0.3)
                    next_key += 1
                    last_key = el
                    last_advance = el
                    if ed_seen > 3 or onjs_seen > 3 or pm_seen > 3:
                        print(f"DIALOGUE REACHED via key {sweep[next_key - 1]}; stopping early")
                        break
                if grid and sent_title_keys and el > 8 and el - last_click >= 1.0 and next_cell < len(cells) * 3:
                    if not fg_done:
                        mine = [w for w in windows_of(pid) if w["visible"]]
                        if mine:
                            u32.SetForegroundWindow(mine[0]["hwnd"])
                            time.sleep(0.3)
                        fg_done = True
                    xy = None
                    if not (title_seen and el > title_t + 4.0):
                        last_click = el  # hold: title still transitioning
                    elif not title_clicked or (not after_seen and el - title_click_t > 30.0):
                        title_clicked = True  # title entry (PN_TRACE_ENTRY), retry every 30 s
                        title_click_t = el
                        xy = TITLE_ENTRY
                        print(f"title click {xy} ({_ENTRY}) t={el:.0f}s fetches={drained}")
                    elif after_seen and not menu_clicked:
                        menu_clicked = True
                        menu_click_t = el
                        xy = AFTER_DIARY_MARIA  # measured centre (s7 menu art note + vision pass)
                        print(f"menu click {xy} t={el:.0f}s fetches={drained}")
                    elif menu_clicked and el - menu_click_t < 25.0:
                        last_click = el  # hold: scenario plays on, Space advances
                    elif not after_seen:
                        last_click = el  # hold: Space alone must carry title->after; clicks poison menus
                    else:  # menu clicked, 25 s passed, no dialogue: grid fallback
                        xy = cells[next_cell % len(cells)]
                        next_cell += 1
                    if xy is not None:
                        click(*phys(xy))
                        print(f"click {nclicks} {xy}->{phys(xy)} t={el:.0f}s fetches={drained} ED={ed_seen} ONJS={onjs_seen} PM={pm_seen}")
                        nclicks += 1
                        last_click = el
                    if pm_seen > 3 or ed_seen > 3 or onjs_seen > 3:
                        print(f"DIALOGUE REACHED after click {nclicks - 1}; dumping ring tail")
                        try:
                            tail_n = min(head_now - drained + 200000, NENT)
                            tail_start = head_now - tail_n
                            tail_b = bytearray()
                            dd = tail_start
                            while dd < head_now:
                                idx2 = dd & MASK
                                nn = min(head_now - dd, NENT - idx2)
                                tail_b += read_mem(hp, ring + idx2 * 8, nn * 8)
                                dd += nn
                            (ROOT / "work" / "tmp" / "dialogue_streams.bin").write_bytes(bytes(tail_b))
                            print(f"wrote dialogue_streams.bin ({len(tail_b)} bytes, {len(tail_b)//8} fetches)")
                        except Exception as e:
                            print(f"tail dump failed: {e}")
                        break
                if grid and menu_clicked and not pm_seen and not wedge_saved:
                    # VM thread frozen post-menu (run9: 38.9M frozen 70 s+):
                    # save the wedged streams once so the blocking impl is identifiable
                    if drained != wedge_last:
                        wedge_last, wedge_t = drained, el
                    elif el - wedge_t > 45.0:
                        wedge_saved = True
                        try:
                            wn = min(max(head_now - drained, 0) + 4096, NENT)
                            wb = bytearray()
                            dd = max(head_now - wn, 0)
                            while dd < head_now:
                                idx3 = dd & MASK
                                nn = min(head_now - dd, NENT - idx3)
                                wb += read_mem(hp, ring + idx3 * 8, nn * 8)
                                dd += nn
                            (ROOT / "work" / "tmp" / "wedge.bin").write_bytes(bytes(wb))
                            tail_ops = [(struct.unpack_from("<I", wb, i * 8)[0], wb[i * 8 + 4])
                                        for i in range(max(len(wb) // 8 - 8, 0), len(wb) // 8)]
                            print(f"WEDGE: fetches frozen {el - wedge_t:.0f}s; last streams: "
                                  + " ".join(f"{s:#x}:{o:02X}" for s, o in tail_ops))
                        except Exception as e:
                            print(f"wedge dump failed: {e}")
                if int(el) % 10 == 0:
                    print(f"t={int(el)}s fetches={drained} bytes={len(trace)} ED={ed_seen} ONJS={onjs_seen} PM={pm_seen} clicks={next_cell}")
                # process died?
                code_out = wintypes.DWORD()
                if k32.GetExitCodeProcess(hp, ctypes.byref(code_out)) and code_out.value != 259:
                    print(f"process exited code={code_out.value}")
                    break
            if blt and blt_recs:
                bp = ROOT / "work" / "tmp" / "blt.bin"
                bp.write_bytes(bytes(blt_recs))
                grp = collections.Counter()
                for i in range(0, len(blt_recs) // 16 * 16, 16):
                    fid, hdc, w, h = struct.unpack_from("<IIII", blt_recs, i)
                    grp[(fid, hdc, w, h)] += 1
                print(f"wrote {bp} ({len(blt_recs)} bytes, {len(blt_recs)//16} calls)")
                print("blt groups (fid,hdc,w,h) x n:",
                      " ".join(f"{k}x{v}" for k, v in grp.most_common(12)))
            if peek and ig_peeked:
                # scenario-pointer table (jump base 0x425870, fixed: VC6, no ASLR):
                # find which loaded scenario contains the spin streams
                try:
                    tab = read_mem(hp, 0x425870, 0x400)
                    (ROOT / "work" / "tmp" / "scenario_table.bin").write_bytes(tab)
                    print(f"wrote scenario_table.bin ({len(tab)} bytes)")
                    words = struct.unpack("<64I", tab[:256])
                    lo = min(ig_peeked) - 0x100000
                    print(f"spin streams: {[hex(s) for s in sorted(ig_peeked)]}")
                    for k, w in enumerate(words):
                        if w and lo < w < min(ig_peeked) + 0x1000:
                            print(f"  table[{k}] = {w:#x} (delta to spin: {[hex(s - w) for s in sorted(ig_peeked)]})")
                    if pm_sites or stc_sites:  # which table entry brackets each dialogue site?
                        for tag, sites in (("PM", pm_sites), ("STC", stc_sites)):
                            for sa in sites:
                                cont = [k for k, w in enumerate(words) if w and w <= sa < w + 0x20000]
                                print(f"  {tag}@{sa:#x} under table{cont}")
                except Exception as e:
                    print(f"table dump failed: {e}")
                # heap context around the IG wait site -> work/tmp/spin_heap.bin
                if ig_stream:
                    try:
                        blob = read_mem(hp, ig_stream - 0x2000, 0x4000)
                        hp_path = ROOT / "work" / "tmp" / "spin_heap.bin"
                        hp_path.write_bytes(blob)
                        print(f"heap @{ig_stream:#x}: wrote {hp_path} ({len(blob)} bytes, IG at +0x2000)")
                    except Exception as e:
                        print(f"heap dump failed: {e}")
        finally:
            tail = trace[-160000:] if trace else b""
            out = bytes(head_buf) + b"".join(b for _, b in events) + bytes(tail)
            OUT.write_bytes(out)
            print(f"wrote {OUT} ({len(out)} bytes, kept head+{len(events)} events+tail)")
            print("census:", " ".join(f"{op:02X}x{n}" for op, n in sorted(census.items())))
            # graceful close first (restores video mode), else terminate;
            # in finally so a drain error can't strand the game at 640x480
            try:
                mine = windows_of(pid)
                vis = [w for w in mine if w["visible"]]
                if vis:
                    u32.PostMessageW(vis[0]["hwnd"], 0x0010, 0, 0)
                    k32.WaitForSingleObject(hp, 5000)
            finally:
                k32.TerminateProcess(hp, 0)
                k32.WaitForSingleObject(hp, 5000)
                r = u32.ChangeDisplaySettingsW(None, 0)
                print(f"display_restore={r}")
    finally:
        k32.CloseHandle(ht)
        k32.CloseHandle(hp)


if __name__ == "__main__":
    secs = int(sys.argv[1]) if len(sys.argv) > 1 and sys.argv[1].isdigit() else 40
    main(secs, grid="grid" in sys.argv, peek="peek" in sys.argv, keys="keys" in sys.argv,
         blt="blt" in sys.argv, defocus="defocus" in sys.argv)
