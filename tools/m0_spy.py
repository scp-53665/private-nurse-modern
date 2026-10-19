"""M0 window spy: list top-level windows with class, rect, styles (F15/U4)."""
import ctypes
from ctypes import wintypes

u32 = ctypes.WinDLL("user32", use_last_error=True)
EnumWindows = u32.EnumWindows
EnumWindows.restype = wintypes.BOOL
GetWindowTextLengthW = u32.GetWindowTextLengthW
GetWindowTextW = u32.GetWindowTextW
GetClassNameW = u32.GetClassNameW
GetWindowRect = u32.GetWindowRect
GetWindowThreadProcessId = u32.GetWindowThreadProcessId
GetWindowLongW = u32.GetWindowLongW
IsWindowVisible = u32.IsWindowVisible


def list_windows():
    out = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _):
        pid = wintypes.DWORD()
        GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        n = GetWindowTextLengthW(hwnd)
        buf = ctypes.create_unicode_buffer(n + 1)
        GetWindowTextW(hwnd, buf, n + 1)
        cls = ctypes.create_unicode_buffer(256)
        GetClassNameW(hwnd, cls, 256)
        r = wintypes.RECT()
        GetWindowRect(hwnd, ctypes.byref(r))
        out.append({
            "hwnd": hwnd, "pid": pid.value,
            "visible": bool(IsWindowVisible(hwnd)),
            "title": buf.value, "class": cls.value,
            "rect": (r.left, r.top, r.right, r.bottom),
            "style": GetWindowLongW(hwnd, -16) & 0xFFFFFFFF,
            "exstyle": GetWindowLongW(hwnd, -20) & 0xFFFFFFFF,
        })
        return True

    assert EnumWindows(cb, 0), "EnumWindows failed"
    assert out, "no top-level windows?"
    return out


if __name__ == "__main__":
    rows = list_windows()
    print(f"windows={len(rows)} visible={sum(1 for r in rows if r['visible'])}")
    for r in sorted(rows, key=lambda x: x["pid"])[:40]:
        print(f"{r['pid']:>6} hwnd={r['hwnd']:>8} vis={int(r['visible'])} "
              f"cls={r['class']!r:.40} title={r['title']!r:.60} rect={r['rect']}")
