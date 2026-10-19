"""GDI StretchBlt cost: a 640x480 32bpp DIB stretched into DIB destinations of growing size,
HALFTONE (smooth) vs COLORONCOLOR (nearest). The sink's per-present cost at resolutions
this desktop cannot show (L2-M4, s13). usage: python -I tools/gdi_bench.py"""
import ctypes, time, random
from ctypes import wintypes as wt
g = ctypes.WinDLL("gdi32"); u = ctypes.WinDLL("user32")
g.CreateDIBSection.restype = wt.HANDLE
g.CreateCompatibleDC.restype = wt.HANDLE
g.SelectObject.restype = wt.HANDLE
g.SelectObject.argtypes = [wt.HANDLE, wt.HANDLE]
g.StretchBlt.argtypes = [wt.HANDLE] + [ctypes.c_int] * 4 + [wt.HANDLE] + [ctypes.c_int] * 4 + [wt.DWORD]
g.SetStretchBltMode.argtypes = [wt.HANDLE, ctypes.c_int]
g.SetBrushOrgEx.argtypes = [wt.HANDLE, ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
g.GdiFlush.restype = wt.BOOL

class BIH(ctypes.Structure):
    _fields_ = [("biSize", wt.DWORD), ("biWidth", wt.LONG), ("biHeight", wt.LONG),
                ("biPlanes", wt.WORD), ("biBitCount", wt.WORD), ("biCompression", wt.DWORD),
                ("biSizeImage", wt.DWORD), ("biXPelsPerMeter", wt.LONG),
                ("biYPelsPerMeter", wt.LONG), ("biClrUsed", wt.DWORD), ("biClrImportant", wt.DWORD)]

def dib(w, h):
    bi = BIH(ctypes.sizeof(BIH), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    bits = ctypes.c_void_p()
    hb = g.CreateDIBSection(None, ctypes.byref(bi), 0, ctypes.byref(bits), None, 0)
    dc = g.CreateCompatibleDC(None)
    g.SelectObject(dc, hb)
    return dc, bits, w * h * 4

src, sbits, n = dib(640, 480)
ctypes.memmove(sbits, bytes(random.getrandbits(8) for _ in range(n)), n)
HALFTONE, COLORONCOLOR, SRCCOPY = 4, 3, 0x00CC0020
for (w, h, label) in [(921, 691, "fit@150%virt"), (1440, 1080, "1080p 4:3"),
                      (1920, 1440, "1440p 4:3"), (2880, 2160, "4K 4:3")]:
    dst, _, _ = dib(w, h)
    for mode, mname in ((HALFTONE, "HALFTONE"), (COLORONCOLOR, "COLORONCOLOR")):
        g.SetStretchBltMode(dst, mode); g.SetBrushOrgEx(dst, 0, 0, None)
        ts = []
        for _ in range(60):
            t = time.perf_counter()
            g.StretchBlt(dst, 0, 0, w, h, src, 0, 0, 640, 480, SRCCOPY); g.GdiFlush()
            ts.append((time.perf_counter() - t) * 1000)
        ts.sort()
        print(f"{label:14s} {w}x{h} {mname:12s} p50={ts[30]:6.2f} ms max={ts[-1]:6.2f} ms")
