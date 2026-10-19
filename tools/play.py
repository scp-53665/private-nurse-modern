"""Drive the staging game through a dialogue and take external screenshots (presenter
acceptance, session 13).

Launch -> title keys -> "Continue" -> Return on the load dialog (Data 1) -> advance the
text with mouse clicks -> `tools/shot.py` every SHOT_EVERY seconds. No VM patching
(that is `trace_vm.py`'s job); this is the player's view.

The engine waits for dialogue input in its `IG` op ("in-screen mouse input", E18 0x85):
a **click** on the game window advances the text, Space/Return do not (pb19/pb20).

usage: python tools/play.py SECS TAG [ADVANCE_EVERY=4] [SHOT_EVERY=10]
Clicks map logical 640x480 points through the layout read live from the game window's
client (origin, scale, fullscreen letterbox), so no PN_TRACE_* env is needed and the
driver works for DPI-aware and unaware games alike (it runs DPI-aware itself).
env:          PLAY_PTS="x,y;x,y"   logical click points, cycled (default frame, then box area)
       PLAY_MOVE=1          at t=70 s: move the game window, minimise, restore (shots
                            TAG_moved / TAG_restored) - the one-window acceptance
       PLAY_MENU=1          at t=40 s: the in-game right-click menu (user report, s14): logs the
                            menu rect vs the cursor, opens Properties/LOAD/SAVE (rect vs
                            the screen), toggles Window mode twice; full-screen shots
                            TAG_menu / TAG_<dialog> / TAG_toggle<n>.png, work area each time
       PLAY_DEFOCUS=1       at t=45 s: hand the foreground to the desktop shell and back; prints
                            the game window's WS_EX_TOPMOST bit (expect 1 -> 0 -> 1)
Shots land in work/tmp/TAG_<t>.bmp. The game is killed at the end.
"""
import ctypes
import os
import subprocess
import sys
import time
from pathlib import Path

ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # PMv2: physical px

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
from m0_run import click, press_key, u32, windows_of  # noqa: E402
import trace_vm  # noqa: E402  (phys(), steal_foreground(), TITLE_CONTINUE)

VK_RETURN, VK_SPACE, VK_ESCAPE = 0x0D, 0x20, 0x1B
WM_CLOSE, WM_COMMAND = 0x0010, 0x0111
# Menu 131 (the right-click popup) command ids, from the EXE's RT_MENU resource.
MENU_CMDS = (("properties", 32775), ("load", 32773), ("save", 32774))
CMD_WINDOW_MODE = 2001


def game_window(pid):
    big = [w for w in windows_of(pid) if w["visible"] and w["rect"][3] - w["rect"][1] > 300]
    return big[0]["hwnd"] if big else None


def game_has_focus(hwnd):
    """Review s12/s13 P3-3: input goes to absolute screen coordinates, so send it only
    while the game owns the foreground (else it would hit the user's own windows)."""
    pid = ctypes.c_ulong(0)
    u32.GetWindowThreadProcessId(u32.GetForegroundWindow(), ctypes.byref(pid))
    if pid.value == PID:
        return True
    trace_vm.steal_foreground(hwnd)
    u32.GetWindowThreadProcessId(u32.GetForegroundWindow(), ctypes.byref(pid))
    return pid.value == PID


def shot(tag):
    r = subprocess.run([sys.executable, str(ROOT / "tools/shot.py"),
                        str(ROOT / f"work/tmp/{tag}.bmp")], capture_output=True, text=True)
    return (r.stdout.strip() or r.stderr.strip())[:140]


def live_layout(hwnd):
    """The presenter's frame placement: largest 4:3 fit, centred in the client."""
    pt = (ctypes.c_long * 2)(0, 0)
    rc = (ctypes.c_long * 4)()
    u32.ClientToScreen(ctypes.c_void_p(hwnd), pt)
    u32.GetClientRect(ctypes.c_void_p(hwnd), rc)
    w, h = rc[2], rc[3]
    if w < 64 or h < 48:  # minimised or not yet sized: keep the last layout
        return
    s = min(w / 640, h / 480)
    trace_vm.LAYOUT_SCALE = s
    trace_vm.LAYOUT_ORIGIN = (pt[0] + round((w - 640 * s) / 2), pt[1] + round((h - 480 * s) / 2))


def move_test(hwnd, tag):
    """A user-style move, minimise and restore of the game window (not the engine's)."""
    u32.SetWindowPos(hwnd, 0, 200, 60, 0, 0, 0x0001 | 0x0004 | 0x0010)  # NOSIZE|NOZORDER|NOACTIVATE
    time.sleep(1.5)
    print(f"moved: {shot(tag + '_moved')}")
    u32.ShowWindow(hwnd, 6)  # SW_MINIMIZE
    time.sleep(1.5)
    print("minimised; visible:", [(hex(w["hwnd"]), w["rect"]) for w in windows_of(PID) if w["visible"]])
    u32.ShowWindow(hwnd, 9)  # SW_RESTORE
    trace_vm.steal_foreground(hwnd)
    time.sleep(1.5)
    print(f"restored: {shot(tag + '_restored')}")
    u32.SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0001 | 0x0004 | 0x0010)


def grab(tag):
    from PIL import ImageGrab
    ImageGrab.grab(all_screens=True).save(ROOT / f"work/tmp/{tag}.png")


def work_area():
    r = (ctypes.c_long * 4)()
    u32.SystemParametersInfoW(0x30, 0, r, 0)  # SPI_GETWORKAREA
    return tuple(r)


def visible(hwnd, cls=None):
    return [w for w in windows_of(PID) if w["visible"] and w["hwnd"] != hwnd
            and (cls is None or w["class"] == cls)]


def menu_test(hwnd, tag):
    at = trace_vm.phys((320, 240))
    # The right-click goes to absolute screen coordinates, so it must not run unless the
    # game owns the foreground: otherwise it lands on the user's own window (s14b m-4).
    if game_has_focus(hwnd):
        u32.SetCursorPos(*at)
        time.sleep(0.2)
        u32.mouse_event(0x0008, 0, 0, 0, 0)  # RIGHTDOWN
        time.sleep(0.05)
        u32.mouse_event(0x0010, 0, 0, 0, 0)  # RIGHTUP
    time.sleep(1.0)
    print(f"menu cursor={at} menus={[w['rect'] for w in visible(hwnd, '#32768')]}")
    grab(tag + "_menu")
    press_key(VK_ESCAPE)
    time.sleep(0.8)
    for name, cmd in MENU_CMDS:
        u32.PostMessageW(hwnd, WM_COMMAND, cmd, 0)
        time.sleep(1.5)
        dlg = visible(hwnd, "#32770")
        print(f"{name}: game={u32_rect(hwnd)} dialogs={[(d['title'], d['rect']) for d in dlg]}")
        grab(f"{tag}_{name}")
        for d in dlg:
            u32.PostMessageW(d["hwnd"], WM_CLOSE, 0, 0)
        time.sleep(1.0)
    for i in (1, 2):
        u32.PostMessageW(hwnd, WM_COMMAND, CMD_WINDOW_MODE, 0)
        time.sleep(2.5)
        print(f"toggle{i}: game={u32_rect(hwnd)} others={[(w['class'], w['rect']) for w in visible(hwnd)]}"
              f" workarea={work_area()}")
        grab(f"{tag}_toggle{i}")


def u32_rect(hwnd):
    r = (ctypes.c_long * 4)()
    u32.GetWindowRect(ctypes.c_void_p(hwnd), r)
    return tuple(r)


def defocus_test(hwnd):
    """Alt-tab acceptance (review s12/s13 P2-4): topmost must drop while away."""
    def topmost():
        return int(bool(u32.GetWindowLongW(ctypes.c_void_p(hwnd), -20) & 0x8))
    before = topmost()
    trace_vm.steal_foreground(u32.GetShellWindow())
    time.sleep(1.5)
    away = topmost()
    trace_vm.steal_foreground(hwnd)
    time.sleep(1.5)
    print(f"defocus topmost before={before} away={away} back={topmost()}")


def main(secs, tag, adv_every, shot_every):
    global PID
    pts = [tuple(int(v) for v in q.split(","))
           for q in os.environ.get("PLAY_PTS", "320,200;320,430").split(";")]
    p = subprocess.Popen([str(ROOT / "work/run/NURSEUS.EXE")], cwd=str(ROOT / "work/run"))
    PID = p.pid
    t0 = time.time()
    keys = moved = defocused = menu = False
    clicked = ok = 0.0  # times (s) of the last Continue click / the load OK
    main_hwnd = None
    last_adv = last_shot = 0.0
    n = 0
    try:
        while (el := time.time() - t0) < secs and p.poll() is None:
            time.sleep(0.2)
            # The first big window is the game; later ones may be the engine's dialogs.
            # Re-acquired if it died: a stale HWND makes every click a silent no-op and a
            # run that looks successful while testing nothing (s14b m-5).
            if not main_hwnd or not u32.IsWindow(ctypes.c_void_p(main_hwnd)):
                main_hwnd = game_window(p.pid)
            hwnd = main_hwnd
            if hwnd:  # the window keeps the engine's x,y (review P2-3): never a fixed origin
                live_layout(hwnd)
            if el > 3 and not keys and hwnd:
                trace_vm.steal_foreground(hwnd)
                for vk in (VK_RETURN, VK_SPACE, VK_RETURN):  # skip the splash
                    if game_has_focus(hwnd):
                        press_key(vk)
                    time.sleep(0.4)
                keys = True
            # Continue opens the "Data" #32770 load dialog. The title menu goes live a
            # variable time after the splash (run vd4: ~2 s late, the click was lost), so
            # re-click every 3 s until the dialog exists, then Return on it.
            dialog = clicked and not ok and visible(hwnd, "#32770")
            if (el > 14 and not ok and not dialog and el - clicked >= 3 and hwnd
                    and game_has_focus(hwnd)):
                click(*trace_vm.phys(trace_vm.TITLE_CONTINUE))
                clicked = el
                print(f"t={el:.0f} continue")
            if dialog and game_has_focus(hwnd):
                press_key(VK_RETURN)  # load dialog: Data 1 is preselected -> OK
                ok = el
            if ok and el > ok + 4 and el - last_adv >= adv_every and hwnd and game_has_focus(hwnd):
                click(*trace_vm.phys(pts[n % len(pts)]))
                n += 1
                last_adv = el
            if os.environ.get("PLAY_MENU") and hwnd and el > 40 and not menu:
                menu = True
                menu_test(hwnd, tag)
            if os.environ.get("PLAY_DEFOCUS") and hwnd and el > 45 and not defocused:
                defocused = True
                defocus_test(hwnd)
            if os.environ.get("PLAY_MOVE") and hwnd and el > 70 and not moved:
                moved = True
                move_test(hwnd, tag)
            if el - last_shot >= shot_every:
                last_shot = el
                print(f"t={el:.0f} {shot(f'{tag}_{int(el):03d}')}")
    finally:
        alive = p.poll() is None
        p.kill()
        print(f"end alive={alive} advances={n}")


if __name__ == "__main__":
    a = sys.argv[1:]
    main(int(a[0]), a[1], float(a[2]) if len(a) > 2 else 4.0, float(a[3]) if len(a) > 3 else 10.0)
