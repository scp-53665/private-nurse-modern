//! Frame read-back and dump: what the engine drew (source DC) and what is on screen
//! (window DC), as 32-bit pixels, hashed per call and written as 24-bit BMPs.
//!
//! The read-back deliberately goes through `GetDIBits` (the Phase-B pipeline, review
//! R4): a DIB section's colour table is what the game set with `SetDIBColorTable`, and
//! this path proves the colour conversion works on the live engine.

// Win32 ABI arithmetic (pixel counts, DIB field widening, BMP header packing).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetCurrentObject, GetDIBits, GetObjectW, OBJ_BITMAP, SRCCOPY, SelectObject,
};

use crate::log;
use crate::win::{BITMAPINFO, BITMAPINFOHEADER, DIBSECTION, GetDC, HBITMAP, HDC, HWND, ReleaseDC};

/// Largest bitmap the spike will copy (a 640×480 engine, generous headroom).
const MAX_SIDE: i32 = 4096;
/// Unchanged screens still get one sample per this interval, so a static phase is a
/// time series and not a single frame.
const HEARTBEAT_MS: u64 = 30_000;
/// Minimum gap between dumps: a fade or an animated screen must not eat the whole
/// budget before the interesting phase starts (measured: the first title fade alone
/// produced ~30 unique frames within two seconds).
const MIN_GAP_MS: u64 = 1_000;
/// Minimum gap for a flat (fade / fill) frame: slower, because a fade is one event
/// spread over many frames.
const FLAT_GAP_MS: u64 = 5_000;
/// Distinct-byte count above which a frame counts as "art" and is always dumped.
const RICH_BYTES: usize = 8;

/// Cheap "is there any art on this frame" proxy: distinct byte values in a strided
/// sample. Flat fades and single-colour fills score 1-2.
fn richness(pixels: &[u8]) -> usize {
    let mut seen = [false; 256];
    let mut n = 0;
    for &b in pixels.iter().step_by(16) {
        if !seen[b as usize] {
            seen[b as usize] = true;
            n += 1;
            if n >= RICH_BYTES {
                break;
            }
        }
    }
    n
}

#[derive(Debug, Clone)]
pub struct Capture {
    pub w: i32,
    pub h: i32,
    pub bpp: u16,
    pub hash: u64,
    pub file: Option<String>,
    pub us: u64,
}

static PIX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Census/window sampling keeps its own buffer so a slow `PrintWindow` never blocks
/// the present path on the pixel lock (review N6).
static CENSUS_PIX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static DUMPED: AtomicU32 = AtomicU32::new(0);
/// Text-box frames, capped separately (see `plan_dump`).
const MAX_BOX_SHOTS: u32 = 60;
static BOX_SHOTS: AtomicU32 = AtomicU32::new(0);
static SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static LAST_FORCE_MS: AtomicU64 = AtomicU64::new(0);
static LAST_DUMP_MS: AtomicU64 = AtomicU64::new(0);

pub fn dumped() -> u32 {
    DUMPED.load(Ordering::Relaxed)
}

fn fnv1a(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h ^= u64::from(c);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Reads the bitmap currently selected into `hdc` as top-down BGRX.
///
/// # Safety
/// `hdc` must be a valid DC handle in this process.
unsafe fn read_into(dst: &mut Vec<u8>, hdc: HDC) -> Option<(i32, i32, u16)> {
    // SAFETY: hdc is caller-supplied; GetCurrentObject tolerates a null bitmap.
    let hbmp: HBITMAP = unsafe { GetCurrentObject(hdc, OBJ_BITMAP as u32) }.cast();
    if hbmp.is_null() {
        return None;
    }
    let mut ds: DIBSECTION = unsafe { std::mem::zeroed() };
    // SAFETY: ds is a correctly sized DIBSECTION for the GetObjectW call.
    let got = unsafe { GetObjectW(hbmp, size_of::<DIBSECTION>() as i32, (&raw mut ds).cast()) };
    if got == 0 {
        return None;
    }
    let (w, h) = (ds.dsBm.bmWidth, ds.dsBm.bmHeight);
    if w <= 0 || h <= 0 || w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    let (wu, hu) = (w as usize, h as usize);
    dst.clear();
    dst.resize(wu * hu * 4, 0);
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = 0; // BI_RGB
    // SAFETY: dst is w*h*4 bytes and bmi describes exactly that layout.
    let lines = unsafe {
        GetDIBits(
            hdc,
            hbmp,
            0,
            h as u32,
            dst.as_mut_ptr().cast(),
            &raw mut bmi,
            DIB_RGB_COLORS,
        )
    };
    if lines == 0 && !copy_bits(dst, &ds, wu, hu) {
        return None;
    }
    Some((w, h, ds.dsBm.bmBitsPixel))
}

/// Fallback for a `GetDIBits` refusal: copy the DIB section's own bits (32 bpp only;
/// an 8-bpp DIB needs the GDI conversion that just failed).
fn copy_bits(dst: &mut [u8], ds: &DIBSECTION, w: usize, h: usize) -> bool {
    if ds.dsBm.bmBits.is_null() || ds.dsBm.bmBitsPixel != 32 {
        return false;
    }
    let src = ds.dsBm.bmBits.cast::<u8>();
    let bottom_up = ds.dsBmih.biHeight > 0;
    for y in 0..h {
        let sy = if bottom_up { h - 1 - y } else { y };
        // SAFETY: the DIB section is w*h*4 bytes per bmWidth/bmHeight/bmBitsPixel.
        let row = unsafe { std::slice::from_raw_parts(src.add(sy * w * 4), w * 4) };
        dst[y * w * 4..(y + 1) * w * 4].copy_from_slice(row);
    }
    true
}

fn write_bmp(name: &str, pixels: &[u8], w: i32, h: i32) -> Option<String> {
    let path = log::frame_path(name)?;
    let (wu, hu) = (w as usize, h as usize);
    let stride = (wu * 3).div_ceil(4) * 4;
    let size = stride * hu;
    let mut out = Vec::with_capacity(54 + size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&u32::try_from(54 + size).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(size).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..hu).rev() {
        for x in 0..wu {
            let p = &pixels[y * wu * 4 + x * 4..];
            out.extend_from_slice(&p[..3]);
        }
        out.resize(out.len() + (stride - wu * 3), 0);
    }
    std::fs::write(&path, out).ok()?;
    Some(name.to_owned())
}

/// Hash in hand: decides whether to dump (rate policy, cap, dedupe) and reserves the
/// file name. The caller writes the file **after** dropping the pixel lock.
///
/// Rate policy (measured need: the first title fade alone produced ~30 unique frames
/// in two seconds and ate the whole budget before the menu phase):
/// art frames at most one per second, flat frames at most one per [`FLAT_GAP_MS`],
/// a forced sample per heartbeat, everything deduped by hash unless forced.
fn plan_dump(pixels: &[u8], w: i32, h: i32, hash: u64, label: &str) -> Option<String> {
    // The dialogue text box (`census::sample_present_target`, label "box") has its own
    // budget: a long run's blit samples must not spend the frames before the dialogue
    // starts (observed in pb9: the shared 60-frame budget was gone by t=60s, so the box's
    // samples were logged but never dumped).
    let reserved = label.starts_with("box");
    if !log::dump_enabled()
        || (!reserved && DUMPED.load(Ordering::Relaxed) >= log::max_frames())
        || (reserved && BOX_SHOTS.load(Ordering::Relaxed) >= MAX_BOX_SHOTS)
    {
        return None;
    }
    if reserved {
        BOX_SHOTS.fetch_add(1, Ordering::Relaxed);
    }
    let now = log::ms();
    let since = now.saturating_sub(LAST_DUMP_MS.load(Ordering::Relaxed));
    let force = now.saturating_sub(LAST_FORCE_MS.load(Ordering::Relaxed)) >= HEARTBEAT_MS;
    if !force {
        let art = richness(pixels) >= RICH_BYTES;
        let gap = if art { MIN_GAP_MS } else { FLAT_GAP_MS };
        if since < gap {
            return None;
        }
        let seen = SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.contains(&hash) {
            return None;
        }
    }
    let idx = DUMPED.fetch_add(1, Ordering::Relaxed) + 1;
    let name = format!("{idx:04}_{label}_{w}x{h}_{:08x}.bmp", hash as u32);
    {
        let mut seen = SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.len() < 8192 && !seen.contains(&hash) {
            seen.push(hash);
        }
    }
    LAST_DUMP_MS.store(now, Ordering::Relaxed);
    if force {
        LAST_FORCE_MS.store(now, Ordering::Relaxed);
    }
    Some(name)
}

/// Writes a reserved frame and logs it. Called with the pixel lock released.
fn finish_dump(name: &str, pixels: &[u8], w: i32, h: i32) {
    match write_bmp(name, pixels, w, h) {
        Some(file) => log::line(
            "frame",
            format_args!("dumped {file} {w}x{h} bytes={}", pixels.len()),
        ),
        None => log::line("frame", format_args!("dump FAILED {name} {w}x{h}")),
    }
}

/// Reads a DC's current bitmap, hashes it and dumps it if new.
///
/// # Safety
/// `hdc` must be a valid DC handle in this process.
pub unsafe fn observe_dc(hdc: HDC, label: &str) -> Option<Capture> {
    if hdc.is_null() {
        return None;
    }
    let t0 = crate::win::qpc_now();
    let (capture, write) = {
        let mut store = PIX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: the lock guard is a Vec<u8> store; hdc is caller-supplied.
        let (w, h, bpp) = unsafe { read_into(&mut store, hdc) }?;
        let hash = fnv1a(&store);
        let plan = plan_dump(&store, w, h, hash, label);
        let write = plan.map(|name| (name, store.clone(), w, h));
        (
            Capture {
                w,
                h,
                bpp,
                hash,
                file: write.as_ref().map(|(n, _, _, _)| n.clone()),
                us: 0,
            },
            write,
        )
    }; // the pixel lock is dropped before any file I/O: a dump must not stall a hook
    if let Some((name, pixels, w, h)) = write {
        finish_dump(&name, &pixels, w, h);
    }
    Some(Capture {
        us: crate::win::qpc_us_since(t0),
        ..capture
    })
}

/// Reads a window's pixels by blitting its **window DC** into a compatible bitmap.
///
/// `PrintWindow` is deliberately *not* used: it sends `WM_PRINT` to the window's thread,
/// and when the engine sits in its non-pumping input wait (measured: the VM freeze the
/// tracer calls "wedge") that message is never processed, so the call blocks the sampler
/// forever — observed in grid4, where the census thread stopped at t=111 s while the game
/// stayed alive to t=400 s (review follow-up, NF-18). `BitBlt` reads the visible surface
/// without touching the engine's message queue.
///
/// # Safety
/// `hwnd` must be a window handle in this process.
pub unsafe fn observe_window(hwnd: HWND, label: &str) -> Option<Capture> {
    if hwnd.is_null() {
        return None;
    }
    let t0 = crate::win::qpc_now();
    // SAFETY: hwnd is caller-supplied; GetDC/companions are the documented pairing.
    let wdc = unsafe { GetDC(hwnd) };
    if wdc.is_null() {
        return None;
    }
    let mut rect = crate::win::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: rect is a valid RECT.
    unsafe { crate::win::GetWindowRect(hwnd, &raw mut rect) };
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    let out = if w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE {
        // SAFETY: paired creation/deletion below.
        unsafe { capture_compatible(wdc, hwnd, w, h, label, t0) }
    } else {
        None
    };
    // SAFETY: pairs the GetDC above.
    unsafe { ReleaseDC(hwnd, wdc) };
    out
}

/// # Safety
/// `wdc` must be a DC obtained for `hwnd`; `w`/`h` are the window's size.
unsafe fn capture_compatible(
    wdc: HDC,
    _hwnd: HWND,
    w: i32,
    h: i32,
    label: &str,
    t0: i64,
) -> Option<Capture> {
    // SAFETY: classic compat-DC recipe; every handle is released before returning.
    unsafe {
        let mem = CreateCompatibleDC(wdc);
        let bmp = CreateCompatibleBitmap(wdc, w, h);
        // Release whichever handle did come up before bailing out.
        if mem.is_null() || bmp.is_null() {
            if !mem.is_null() {
                DeleteDC(mem);
            }
            if !bmp.is_null() {
                DeleteObject(bmp.cast());
            }
            return None;
        }
        let old = SelectObject(mem, bmp.cast());
        // SAFETY: mem holds a w×h bitmap; wdc is this window's DC, read-only here.
        let ok = BitBlt(mem, 0, 0, w, h, wdc, 0, 0, SRCCOPY);
        let out = if ok != 0 {
            read_into_and_dump(mem, label, t0)
        } else {
            None
        };
        SelectObject(mem, old);
        DeleteObject(bmp.cast());
        DeleteDC(mem);
        out
    }
}

/// # Safety
/// `dc` must hold a bitmap of its own size.
unsafe fn read_into_and_dump(dc: HDC, label: &str, t0: i64) -> Option<Capture> {
    // The census uses its own buffer: its `PrintWindow` capture must never stall the
    // present path's hook on the shared pixel lock (review N6).
    let (capture, write) = {
        let mut store = CENSUS_PIX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: forwarded from the caller.
        let (w, h, bpp) = unsafe { read_into(&mut store, dc) }?;
        let hash = fnv1a(&store);
        let write = plan_dump(&store, w, h, hash, label).map(|name| (name, store.clone(), w, h));
        (
            Capture {
                w,
                h,
                bpp,
                hash,
                file: write.as_ref().map(|(n, _, _, _)| n.clone()),
                us: 0,
            },
            write,
        )
    };
    if let Some((name, pixels, w, h)) = write {
        finish_dump(&name, &pixels, w, h);
    }
    Some(Capture {
        us: crate::win::qpc_us_since(t0),
        ..capture
    })
}
