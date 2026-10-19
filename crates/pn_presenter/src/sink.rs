//! The frame-sink seam (`docs/04-l2-presenter.md` §3 directive, §4 ladder).
//!
//! The sink is the presenter's *only* way to put pixels on screen, so the
//! GDI-vs-SDL decision stays a test result instead of a code shape. The GDI sink here
//! scales the composed frame itself (`resample`, into a physical-size DIB copied 1:1) —
//! GDI's `HALFTONE` smoothed some dirty rects and not others (session 15) — and falls
//! back to `StretchBlt` from the source DC for any source that is not one of the
//! presenter's own DIB sections. An SDL sink would hand the same pixels to a renderer.
//! Neither the geometry module nor the window policy knows which one is installed.

// HDC/HWND arithmetic and GDI rect packing.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::sync::Mutex;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACKNESS, BitBlt, COLORONCOLOR, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetCurrentObject, HALFTONE,
    HGDIOBJ, OBJ_BITMAP, PatBlt, SRCCOPY, SelectObject, SetBrushOrgEx, SetStretchBltMode,
    StretchBlt,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::config::{self, Filter};
use crate::geom;
use crate::win::{GetDC, HDC, HWND, RECT, ReleaseDC};

/// One region of the engine's frame on its way to the screen.
pub struct Present {
    /// Destination window: the game window or the dialogue text box (NF-12).
    pub hwnd: HWND,
    /// Physical position of the destination's logical (0,0) in the window's **client**
    /// coordinates, the space of `GetDC(hwnd)`
    /// ([`crate::window::client_logical_origin`] — the geometry module's job, not the sink's).
    pub origin: (i32, i32),
    /// Source DC exactly as the engine passed it (memory DC, DIB section, window DC).
    pub src: HDC,
    /// Region of the source to show (`[x, y, w, h]`).
    pub src_rect: [i32; 4],
    /// Where the region belongs in the window's logical client space.
    pub logical: [i32; 4],
}

pub trait FrameSink {
    /// Draws `present` scaled; `false` means "did not present" and the caller lets the
    /// engine's own blit run unchanged.
    fn present(&self, present: &Present) -> bool;
}

/// The GDI-in-place sink (`docs/04` §3).
pub struct GdiSink;

static GDI: GdiSink = GdiSink;

pub fn sink() -> &'static dyn FrameSink {
    &GDI
}

impl FrameSink for GdiSink {
    fn present(&self, present: &Present) -> bool {
        if !present.is_valid() {
            return false;
        }
        // SAFETY: `is_valid` vetted the handle; a vanished window just yields null.
        let window_dc = unsafe { GetDC(present.hwnd) };
        if window_dc.is_null() {
            return false;
        }
        clear_on_layout_change(present.hwnd, window_dc);
        let ok = resampled(present, window_dc).unwrap_or_else(|| stretch(present, window_dc));
        // SAFETY: pairs the GetDC above.
        unsafe { ReleaseDC(present.hwnd, window_dc) };
        ok
    }
}

impl Present {
    fn is_valid(&self) -> bool {
        !self.hwnd.is_null()
            && !self.src.is_null()
            && self.logical[2] > 0
            && self.logical[3] > 0
            && self.src_rect[2] > 0
            && self.src_rect[3] > 0
    }
}

/// A top-down 32-bpp DIB section selected into its own memory DC: `(dc, bits)`. Owned by
/// the caller; GDI writes it through the DC, the presenter through the pointer (after
/// `GdiFlush`).
pub fn new_dib(w: i32, h: i32) -> Option<(HDC, *mut u32)> {
    // SAFETY: a zeroed BITMAPINFO is valid; the header makes it a 32-bpp top-down DIB, and
    // CreateDIBSection returns the bits through `bits`.
    unsafe {
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(
            std::ptr::null_mut(),
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            std::ptr::null_mut(),
            0,
        );
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if bmp.is_null() || dc.is_null() || bits.is_null() {
            free_dib(dc, bmp);
            return None;
        }
        SelectObject(dc, bmp);
        dibs().push((dc as usize, bits as usize, w, h));
        Some((dc, bits.cast()))
    }
}

/// The DIB sections made by [`new_dib`], `(dc, bits, w, h)`: a DC's pixels are read only
/// when the presenter made it (GDI cannot report a DIB's row order back: `GetObject`
/// gives a positive `biHeight` for a top-down DIB, measured session 15).
fn dibs() -> std::sync::MutexGuard<'static, Vec<(usize, usize, i32, i32)>> {
    static DIBS: Mutex<Vec<(usize, usize, i32, i32)>> = Mutex::new(Vec::new());
    DIBS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn free_dib(dc: HDC, bmp: HGDIOBJ) {
    dibs().retain(|d| d.0 != dc as usize);
    // SAFETY: handles we created; nulls are skipped.
    unsafe {
        let bmp = if bmp.is_null() && !dc.is_null() {
            GetCurrentObject(dc, OBJ_BITMAP as u32)
        } else {
            bmp
        };
        if !dc.is_null() {
            DeleteDC(dc); // releases the selection, so the bitmap can be deleted
        }
        if !bmp.is_null() {
            DeleteObject(bmp);
        }
    }
}

/// The bits of a DIB section the presenter made: `(bits, width, height)`.
fn dib_bits(dc: HDC) -> Option<(*const u32, usize, usize)> {
    dibs()
        .iter()
        .find(|d| d.0 == dc as usize)
        .map(|d| (d.1 as *const u32, d.2 as usize, d.3 as usize))
}

/// The scaled frame (physical size), rebuilt when the plan's size changes.
struct Out {
    dc: usize,
    bits: usize,
    w: i32,
    h: i32,
}

static OUT: Mutex<Option<Out>> = Mutex::new(None);

/// The physical frame buffer for a `w x h` frame, (re)built on a size change.
fn out_for(slot: &mut Option<Out>, w: i32, h: i32) -> Option<&Out> {
    if slot.as_ref().is_none_or(|o| (o.w, o.h) != (w, h)) {
        if let Some(old) = slot.take() {
            free_dib(old.dc as HDC, std::ptr::null_mut());
        }
        let Some((dc, bits)) = new_dib(w, h) else {
            // Every present falls back to GDI's stretch (dirty rects may differ again).
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                crate::log::line("present", format_args!("scaler frame {w}x{h} FAILED"));
            });
            return None;
        };
        crate::log::line("present", format_args!("scaler frame {w}x{h}"));
        *slot = Some(Out {
            dc: dc as usize,
            bits: bits as usize,
            w,
            h,
        });
    }
    slot.as_ref()
}

/// The presenter's own scaling ([`crate::resample`]): `present.logical` of a frame-sized
/// DIB-section source is resampled into the physical frame and copied 1:1. `None`: the
/// source is not such a DIB (the caller stretches with GDI instead).
fn resampled(present: &Present, window_dc: HDC) -> Option<bool> {
    let (bits, sw, sh) = dib_bits(present.src)?;
    let frame = (geom::FRAME.0 as usize, geom::FRAME.1 as usize);
    if (sw, sh) != frame || present.src_rect != present.logical {
        return None;
    }
    let plan = geom::plan();
    let full = geom::rect_to_physical((0, 0), plan.scale, crate::virt::SCREEN);
    let mut slot = OUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let o = out_for(&mut slot, full[2], full[3])?;
    // A bilinear tap reaches one source pixel past the region: grow it by one.
    let l = present.logical;
    let d = geom::rect_to_physical((0, 0), plan.scale, [l[0] - 1, l[1] - 1, l[2] + 2, l[3] + 2]);
    let (x0, y0) = (d[0].max(0), d[1].max(0));
    let (x1, y1) = ((d[0] + d[2]).min(o.w), (d[1] + d[3]).min(o.h));
    if x1 <= x0 || y1 <= y0 {
        return Some(true);
    }
    let smooth = geom::filter_for(&plan, config::current().filter) != Filter::Nearest;
    let rect = [
        x0 as usize,
        y0 as usize,
        (x1 - x0) as usize,
        (y1 - y0) as usize,
    ];
    // SAFETY: GdiFlush finishes GDI's writes to the source; both buffers are live DIB
    // sections of the sizes given (the source checked by `dib_bits`, `o` built at w x h).
    unsafe {
        GdiFlush();
        let src = std::slice::from_raw_parts(bits, sw * sh);
        let dst = std::slice::from_raw_parts_mut(o.bits as *mut u32, (o.w * o.h) as usize);
        crate::resample::resample(
            src,
            (sw, sh),
            dst,
            (o.w as usize, o.h as usize),
            rect,
            smooth,
        );
    }
    let (ox, oy) = present.origin;
    // SAFETY: both DCs are live; a plain 1:1 copy.
    let ok = unsafe {
        BitBlt(
            window_dc,
            ox + x0,
            oy + y0,
            x1 - x0,
            y1 - y0,
            o.dc as HDC,
            x0,
            y0,
            SRCCOPY,
        )
    };
    Some(ok != 0)
}

/// Scales `present.logical` into the window DC with GDI's own stretch: the fallback for a
/// source that is not a frame-sized DIB section.
///
/// The stretch mode is set and restored per call: the destination is the *engine's* own
/// window DC, whose mode the engine sets for its own `StretchBlt` calls (`E02`).
fn stretch(present: &Present, window_dc: HDC) -> bool {
    let plan = geom::plan();
    let dst = geom::rect_to_physical(present.origin, plan.scale, present.logical);
    let filter = geom::filter_for(&plan, config::current().filter);
    let mode = match filter {
        Filter::Nearest => COLORONCOLOR,
        _ => HALFTONE,
    };
    // SAFETY: window_dc is a live DC; SetStretchBltMode returns the previous mode.
    let previous = unsafe { SetStretchBltMode(window_dc, mode) };
    if mode == HALFTONE {
        // One brush origin for every region, or neighbouring tiles get seams.
        // SAFETY: null out-parameter is allowed.
        unsafe { SetBrushOrgEx(window_dc, 0, 0, std::ptr::null_mut()) };
    }
    // SAFETY: both DCs come from the hooked call; the rects were validated above.
    let ok = unsafe {
        StretchBlt(
            window_dc,
            dst[0],
            dst[1],
            dst[2],
            dst[3],
            present.src,
            present.src_rect[0],
            present.src_rect[1],
            present.src_rect[2],
            present.src_rect[3],
            SRCCOPY,
        )
    };
    // SAFETY: restores the mode the engine expects on its own DC.
    unsafe { SetStretchBltMode(window_dc, previous) };
    ok != 0
}

/// Paints the client black the first time a window presents at a new client size, so
/// bars and uncovered corners are never stale garbage. Bounded state: the key is
/// (window, client size) and there are two windows.
fn clear_on_layout_change(hwnd: HWND, window_dc: HDC) {
    static SEEN: std::sync::Mutex<Vec<(usize, i32, i32)>> = std::sync::Mutex::new(Vec::new());
    let (w, h) = client_size(hwnd);
    if w <= 0 || h <= 0 {
        return;
    }
    let mut seen = SEEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let key = (hwnd as usize, w, h);
    if seen.contains(&key) {
        return;
    }
    if seen.len() >= 64 {
        seen.clear(); // handle reuse: the table is a cache, not a log
    }
    seen.push(key);
    drop(seen);
    // SAFETY: BLACKNESS ignores the brush; window_dc is live.
    unsafe { PatBlt(window_dc, 0, 0, w, h, BLACKNESS) };
    crate::log::line(
        "present",
        format_args!("clear hwnd={:#x} {w}x{h}", hwnd as usize),
    );
}

/// Physical client size of a window, straight from USER32 (never the virtualised hook).
pub fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: r is a valid RECT out-parameter.
    unsafe { GetClientRect(hwnd, &raw mut r) };
    (r.right - r.left, r.bottom - r.top)
}
