"""M0 run: launch staging NURSEUS.EXE, poll windows + loaded modules, terminate, diff SAV."""
import ctypes
import hashlib
import subprocess
import sys
import time
from ctypes import wintypes
from pathlib import Path

from m0_spy import list_windows

ROOT = Path(__file__).resolve().parent.parent
RUN = ROOT / "work" / "run"
EXE = RUN / "NURSEUS.EXE"
SAV = RUN / "NURSEUS.SAV"

u32 = ctypes.WinDLL("user32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
KEYEVENTF_KEYUP = 0x0002


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(n, ctypes.c_uint64) for n in
                ("ReadOps", "WriteOps", "OtherOps",
                 "ReadBytes", "WriteBytes", "OtherBytes")]


def press_key(vk):
    u32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.05)
    u32.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)


def grab(tag, hwnd=0):
    try:
        from PIL import ImageGrab
        im = ImageGrab.grab()
        p = RUN / f"m0_{tag}.png"
        im.save(p)
        print(f"shot {p.name} {im.size}")
    except Exception as e:
        print(f"shot {tag} failed: {e}")
    if hwnd:
        try:
            from PIL import Image
            g32 = ctypes.WinDLL("gdi32", use_last_error=True)
            w, h = 640, 480
            hdc_w = u32.GetWindowDC(hwnd)
            hdc_m = g32.CreateCompatibleDC(hdc_w)
            import ctypes.wintypes as wt
            class BITMAPINFOHEADER(ctypes.Structure):
                _fields_ = [("biSize", wt.DWORD), ("biWidth", wt.LONG),
                            ("biHeight", wt.LONG), ("biPlanes", wt.WORD),
                            ("biBitCount", wt.WORD), ("biCompression", wt.DWORD),
                            ("biSizeImage", wt.DWORD), ("biXPelsPerMeter", wt.LONG),
                            ("biYPelsPerMeter", wt.LONG), ("biClrUsed", wt.DWORD),
                            ("biClrImportant", wt.DWORD)]
            class BITMAPINFO(ctypes.Structure):
                _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", wt.DWORD * 3)]
            bmi = BITMAPINFO()
            bmi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
            bmi.bmiHeader.biWidth = w
            bmi.bmiHeader.biHeight = -h
            bmi.bmiHeader.biPlanes = 1
            bmi.bmiHeader.biBitCount = 24
            bmi.bmiHeader.biCompression = 0
            buf = ctypes.create_string_buffer(w * h * 3)
            hbm = g32.CreateCompatibleBitmap(hdc_w, w, h)
            g32.SelectObject(hdc_m, hbm)
            ok = u32.PrintWindow(hwnd, hdc_m, 0)
            g32.GetDIBits(hdc_m, hbm, 0, h, buf, ctypes.byref(bmi), 0)
            g32.DeleteObject(hbm)
            g32.DeleteDC(hdc_m)
            u32.ReleaseDC(hwnd, hdc_w)
            p2 = RUN / f"m0_{tag}.pw.png"
            Image.frombuffer("RGB", (w, h), buf, "raw", "BGR", w * 3, 1).save(p2)
            print(f"printwindow {p2.name} ok={ok}")
        except Exception as e:
            print(f"printwindow {tag} failed: {e}")


def click(x, y):
    u32.SetCursorPos(x, y)
    time.sleep(0.1)
    u32.mouse_event(0x0002, 0, 0, 0, 0)  # LEFTDOWN
    time.sleep(0.05)
    u32.mouse_event(0x0004, 0, 0, 0, 0)  # LEFTUP


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()[:16]


def windows_of(pid):
    return [w for w in list_windows() if w["pid"] == pid]


HMODULE = ctypes.c_void_p
k32.K32EnumProcessModulesEx.argtypes = [wintypes.HANDLE, ctypes.POINTER(HMODULE),
                                        wintypes.DWORD, ctypes.POINTER(wintypes.DWORD),
                                        wintypes.DWORD]
k32.K32EnumProcessModulesEx.restype = wintypes.BOOL
k32.K32GetModuleBaseNameW.argtypes = [wintypes.HANDLE, HMODULE,
                                      wintypes.LPWSTR, wintypes.DWORD]
k32.K32GetModuleBaseNameW.restype = wintypes.DWORD


def proc_modules(h):
    """Base names of the target process's currently loaded modules (needs VM_READ)."""
    buf = (HMODULE * 1024)()
    need = ctypes.c_uint32()
    if not k32.K32EnumProcessModulesEx(h, buf, ctypes.sizeof(buf), ctypes.byref(need),
                                       0x03):  # LIST_MODULES_ALL (32-bit view of WOW64 target)
        return []
    name = ctypes.create_unicode_buffer(260)
    out = []
    for i in range(min(need.value // ctypes.sizeof(HMODULE), len(buf))):
        if k32.K32GetModuleBaseNameW(h, buf[i], name, 260):
            out.append(name.value)
    return out


def main(secs=25):
    assert EXE.exists(), EXE
    before = sha(SAV)
    base = len(list_windows())
    print(f"baseline_windows={base} sav={before}")
    try:
        dpi = ctypes.WinDLL("user32").GetDpiForSystem()
    except Exception:
        dpi = "n/a"
    print(f"system_dpi={dpi} metrics={u32.GetSystemMetrics(0)}x{u32.GetSystemMetrics(1)}")
    p = subprocess.Popen([str(EXE)], cwd=str(RUN))
    print(f"pid={p.pid}")
    h = k32.OpenProcess(0x0400 | 0x0010, False, p.pid)  # QUERY_INFO | VM_READ
    seen = {}  # module base name -> first-seen second (DLL load probe)

    def io():
        c = IO_COUNTERS()
        ok = k32.GetProcessIoCounters(h, ctypes.byref(c)) if h else 0
        return f"rB={c.ReadBytes}" if ok else "io=n/a"
    fg = u32.GetForegroundWindow()
    try:
        for i in range(secs):
            time.sleep(1)
            if p.poll() is not None:
                print(f"exited t={i + 1}s code={p.returncode}")
                break
            mine = windows_of(p.pid)
            vis = [w for w in mine if w["visible"]]
            if i + 1 == 2:
                grab("title", vis[0]["hwnd"] if vis else 0)
            if i + 1 == 3:
                hwnd = vis[0]["hwnd"] if vis else 0
                if hwnd:
                    u32.SetForegroundWindow(hwnd)
                    time.sleep(0.3)
                for vk in (0x0D, 0x20, 0x0D):  # Return, Space, Return: advance past title
                    press_key(vk)
                    time.sleep(0.4)
                print("sent Return/Space/Return")
                grab("after_keys", vis[0]["hwnd"] if vis else 0)
            if i + 1 == 5 and vis:  # click centre: title menus often need mouse
                r = vis[0]["rect"]
                click(r[0] + (r[2] - r[0]) // 2, r[1] + (r[3] - r[1]) // 2)
                time.sleep(0.8)
                print("clicked centre")
                grab("after_click", vis[0]["hwnd"] if vis else 0)
            if i + 1 == 6 and fg:  # alt-tab resilience: drop fg away and back
                u32.SetForegroundWindow(fg)
                time.sleep(0.5)
                mine2 = windows_of(p.pid)
                vis2 = [w for w in mine2 if w["visible"]]
                print(f"defocus: game_visible={len(vis2)} proc_alive={p.poll() is None}")
                if vis2:
                    u32.SetForegroundWindow(vis2[0]["hwnd"])
                    time.sleep(0.5)
                    print("refocused game")
            mine = windows_of(p.pid)
            vis = [w for w in mine if w["visible"]]
            tag = "; ".join(f"{w['class']}:{w['title']!r:.50}:{w['rect']}:"
                            f"style={w['style']:#010x}:ex={w['exstyle']:#010x}"
                            for w in vis[:8])
            print(f"t={i + 1}s total={len(mine)} visible={len(vis)} {io()} {tag}")
            if h:
                for m in proc_modules(h):
                    if m not in seen:
                        seen[m] = i + 1
                        print(f"load t={i + 1}s {m}")
        else:
            print("still running at timeout")
    finally:
        if p.poll() is None:
            mine = windows_of(p.pid)  # prefer graceful close so game restores video mode
            vis = [w for w in mine if w["visible"]]
            if vis:
                u32.PostMessageW(vis[0]["hwnd"], 0x0010, 0, 0)  # WM_CLOSE
                try:
                    print(f"close_wait code={p.wait(timeout=5)}")
                except subprocess.TimeoutExpired:
                    p.terminate()
                    try:
                        p.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        p.kill()
                    print(f"killed code={p.returncode}")
            else:
                p.terminate()
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()
                print(f"killed code={p.returncode}")
        r = u32.ChangeDisplaySettingsW(None, 0)  # game switches to 640x480; restore desktop
        if h:
            k32.CloseHandle(h)
        mx, my = u32.GetSystemMetrics(0), u32.GetSystemMetrics(1)
        print(f"display_restore={r} metrics={mx}x{my}")
    print(f"modules_seen={len(seen)}")
    for m in sorted(seen):
        print(f"  {m}")
    after = sha(SAV)
    print(f"sav_before={before} sav_after={after} changed={before != after}")


if __name__ == "__main__":
    main(int(sys.argv[1]) if len(sys.argv) > 1 else 25)
