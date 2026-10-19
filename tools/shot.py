"""Composited screenshot of the game window (visual-verification helper).

Captures ONLY the game window's own rectangle, never the rest of the desktop:

  1. the foreground window must belong to the staging game (NURSEUS.EXE), and
  2. only that window's physical rect is blitted and written — the desktop is never read
     outside it, and no other window's pixels are copied.

Replaces the old "screen must be <= 800px" guard, which the presenter made obsolete: the
mode switch is suppressed now (the desktop stays at the user's resolution). The capture
must come from a *fresh* process: an in-process screen-DC read is unreliable in this
environment (the first screen shot returned "caption + one strip + black" while the
window DC had the frame).

Physical resolution comes from the display DC (`DESKTOPHORZRES`), because this process is
DPI-unaware: on the dev box `GetSystemMetrics` reports a virtual 1280x720 desktop for a
1920x1080 panel at 150%, and window rectangles are in that virtual space.

Usage: python tools/shot.py [out.bmp]   # default work/tmp/shot_<ts>.bmp
"""
import ctypes
import sys
import time
from ctypes import wintypes
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GAME_EXE = "NURSEUS.EXE"

# wingdi.h DEVICE_CAPABILITIES indices
HORZRES, VERTRES, DESKTOPHORZRES, DESKTOPVERTRES = 8, 10, 118, 117

u32 = ctypes.WinDLL("user32", use_last_error=True)
g32 = ctypes.WinDLL("gdi32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
psapi = ctypes.WinDLL("psapi", use_last_error=True)


def fg_is_game():
    """(ok, reason): foreground window's process image must be the game."""
    hwnd = u32.GetForegroundWindow()
    if not hwnd:
        return False, "no foreground window"
    pid = wintypes.DWORD()
    u32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    hp = k32.OpenProcess(0x410, False, pid.value)  # QUERY_INFORMATION|VM_READ
    if not hp:
        return False, f"pid {pid.value}: OpenProcess failed"
    try:
        buf = ctypes.create_unicode_buffer(260)
        n = psapi.GetModuleBaseNameW(hp, None, buf, 260)
        if not n:
            return False, f"pid {pid.value}: image name unavailable"
        name = buf.value
        if name.lower() != GAME_EXE.lower():
            return False, f"foreground is {name}, not {GAME_EXE}"
        return True, f"foreground pid {pid.value} ({name})"
    finally:
        k32.CloseHandle(hp)


def game_rect():
    """(x, y, w, h) of the foreground (game) window, in this process's units."""
    r = wintypes.RECT()
    if not u32.GetWindowRect(u32.GetForegroundWindow(), ctypes.byref(r)):
        return None
    w, h = r.right - r.left, r.bottom - r.top
    if w <= 0 or h <= 0:
        return None
    return r.left, r.top, w, h


def display_dc_metrics(dc):
    """(virtual, physical) desktop sizes from a display DC."""
    virt = (g32.GetDeviceCaps(dc, HORZRES), g32.GetDeviceCaps(dc, VERTRES))
    phys = (g32.GetDeviceCaps(dc, DESKTOPHORZRES), g32.GetDeviceCaps(dc, DESKTOPVERTRES))
    return virt, phys


class INFO(ctypes.Structure):
    _fields_ = [("size", wintypes.DWORD), ("w", wintypes.LONG),
                ("h", wintypes.LONG), ("planes", wintypes.WORD),
                ("bpp", wintypes.WORD), ("comp", wintypes.DWORD),
                ("sz", wintypes.DWORD), ("xppm", wintypes.LONG),
                ("yppm", wintypes.LONG), ("clr", wintypes.DWORD),
                ("imp", wintypes.DWORD)]


def grab(path, rect, scale):
    """Blit the game window's physical rect from the display DC into a 24-bit BMP."""
    x, y, w, h = (round(rect[0] * scale[0]), round(rect[1] * scale[1]),
                  round(rect[2] * scale[0]), round(rect[3] * scale[1]))
    src = g32.CreateDCW("DISPLAY", None, None, None)
    if not src:
        print("grab: CreateDC(DISPLAY) failed")
        return None
    mem = g32.CreateCompatibleDC(src)
    bmp = g32.CreateCompatibleBitmap(src, w, h)
    old = g32.SelectObject(mem, bmp)
    ok = g32.BitBlt(mem, 0, 0, w, h, src, x, y, 0x00CC0020)  # SRCCOPY
    bi = INFO(ctypes.sizeof(INFO), w, -h, 1, 24, 0, 0, 0, 0, 0, 0)
    stride = (w * 3 + 3) & ~3
    px = ctypes.create_string_buffer(stride * h)
    try:
        if not ok or not g32.GetDIBits(mem, bmp, 0, h, px, ctypes.byref(bi), 0):
            print(f"grab: BitBlt={ok} GetDIBits failed")
            return None
    finally:
        g32.SelectObject(mem, old)
        g32.DeleteObject(bmp)
        g32.DeleteDC(mem)
        g32.DeleteDC(src)
    off = 14 + 40
    with open(path, "wb") as f:
        f.write(b"BM" + (off + stride * h).to_bytes(4, "little")
                + b"\0\0\0\0" + off.to_bytes(4, "little")
                + bytes(bi)[:40] + px.raw)
    return w, h


def main(argv):
    ok, why = fg_is_game()
    if not ok:
        print(f"REFUSE: {why}")
        return 1
    rect = game_rect()
    if rect is None:
        print("REFUSE: no game window rectangle")
        return 1
    tmp = g32.CreateDCW("DISPLAY", None, None, None)
    virt, phys = display_dc_metrics(tmp)
    g32.DeleteDC(tmp)
    scale = (phys[0] / virt[0], phys[1] / virt[1])
    if not (0.2 <= scale[0] <= 8 and 0.2 <= scale[1] <= 8):
        print(f"REFUSE: implausible dpi scale {scale} (virtual {virt}, physical {phys})")
        return 1
    out = Path(argv[1]) if len(argv) > 1 else \
        ROOT / "work" / "tmp" / ("shot_%s.bmp" % time.strftime("%H%M%S"))
    out.parent.mkdir(parents=True, exist_ok=True)
    got = grab(out, rect, scale)
    if got is None:
        return 2
    w, h = got
    print(f"SHOT {out} {w}x{h} window={rect} dpi_scale={scale[0]:.3f} "
          f"({why}; virtual {virt[0]}x{virt[1]}, physical {phys[0]}x{phys[1]})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
