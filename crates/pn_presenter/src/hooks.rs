//! Hook bodies: the present redirect, the Phase A observers, and the hook table.
//!
//! The present path: `BitBlt`/`StretchBlt` into a managed window DC are redirected through
//! `compose` and the frame sink (`present::intercept`). The window and input hooks that
//! keep the engine on its virtual monitor live in `vhooks` (`docs/04` §3.0).
//!
//! Observers (dest/source census, frame CSV, counted high-rate polls) run **only** when
//! `PN_LOG_DIR` is set: a shipped run pays for none of the Phase A instrumentation
//! (NF-15/NF-16 — a 31 kHz poll must not be logged, and the present path must not be
//! sampled per frame).
//!
//! Every hook calls the original through the IAT slot value published by [`crate::pe::install`];
//! a hook that fires with no published original returns a failure value instead of
//! jumping nowhere.

// Win32 ABI arithmetic: handle/integer and 16/32-bit flag conversions are inherent to
// the API surface, and hook signatures must stay parameter-for-parameter identical to
// the imported functions (single-character x/y/w/h included). Allowed as a module,
// justified here once (AGENTS: warn, never deny).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::similar_names
)]

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use windows_sys::Win32::Graphics::Gdi::{
    GetCurrentObject, GetObjectType, OBJ_BITMAP, OBJ_DC, WindowFromDC,
};

use crate::pe::HookSpec;
use crate::win::{BOOL, FALSE, HDC, class_of};
use crate::{frame, log, present};

/// One call site in the CSV: what the engine blitted, where it went, and (when the
/// pixels were read) a hash plus the dump file.
pub fn csv_header() {
    log::csv(format_args!(
        "t_ms,label,src_hdc,src_bmp,bpp,bmp_w,bmp_h,dest_x,dest_y,dest_w,dest_h,\
         src_x,src_y,src_w,src_h,rop,dest_hdc,dest_win,dest_class,hash,dumped,file,us"
    ));
}

/// Reads a function pointer published by the installer.
pub fn orig<T: Copy>(slot: &AtomicUsize) -> Option<T> {
    let v = slot.load(Ordering::Acquire);
    if v == 0 {
        return None;
    }
    // SAFETY: the installer only ever publishes a live pointer of this hook's own
    // signature (or the hook itself, which is filtered out by the "already ours"
    // branch before publishing).
    Some(unsafe { std::mem::transmute_copy::<usize, T>(&v) })
}

/// Runs a rewriting step of a hook; `None` if it panicked, and the caller then forwards
/// the call unchanged. AGENTS hard rule: no unwinding out of an `extern "system"` hook
/// (review s12/s13 P2-6).
pub fn caught<R>(body: impl FnOnce() -> R) -> Option<R> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).ok()
}

/// Runs a hook body, swallowing panics (the process belongs to the game).
pub fn guarded<F: FnOnce()>(what: &str, body: F) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        log::line(
            "panic",
            format_args!("hook {what} panicked; call skipped logging"),
        );
    }
}

// ---------------------------------------------------------------------------
// Present path
// ---------------------------------------------------------------------------

type BitBltFn = unsafe extern "system" fn(HDC, i32, i32, i32, i32, HDC, i32, i32, u32) -> BOOL;
type StretchBltFn =
    unsafe extern "system" fn(HDC, i32, i32, i32, i32, HDC, i32, i32, i32, i32, u32) -> BOOL;

static ORIG_BITBLT: AtomicUsize = AtomicUsize::new(0);
static ORIG_STRETCHBLT: AtomicUsize = AtomicUsize::new(0);
static BLT_SEEN: AtomicUsize = AtomicUsize::new(0);

/// The present redirect, panic-proof (it runs inside the engine's GDI call).
fn try_present(dest: HDC, dst: [i32; 4], src: HDC, src_rect: [i32; 4], rop: u32) -> Option<BOOL> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        present::intercept(dest, dst, src, src_rect, rop)
    }))
    .unwrap_or(None)
}

unsafe extern "system" fn hook_bitblt(
    dest: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: HDC,
    sx: i32,
    sy: i32,
    rop: u32,
) -> BOOL {
    if let Some(presented) = try_present(dest, [x, y, w, h], src, [sx, sy, w, h], rop) {
        return presented;
    }
    if log::spike() {
        guarded("BitBlt", || {
            observe_blt("bb", dest, [x, y, w, h], src, [sx, sy, w, h], rop);
        });
    }
    match orig::<BitBltFn>(&ORIG_BITBLT) {
        Some(f) => unsafe { f(dest, x, y, w, h, src, sx, sy, rop) },
        None => FALSE,
    }
}

unsafe extern "system" fn hook_stretchblt(
    dest: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: HDC,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    rop: u32,
) -> BOOL {
    if let Some(presented) = try_present(dest, [x, y, w, h], src, [sx, sy, sw, sh], rop) {
        return presented;
    }
    if log::spike() {
        guarded("StretchBlt", || {
            observe_blt("sb", dest, [x, y, w, h], src, [sx, sy, sw, sh], rop);
        });
    }
    match orig::<StretchBltFn>(&ORIG_STRETCHBLT) {
        Some(f) => unsafe { f(dest, x, y, w, h, src, sx, sy, sw, sh, rop) },
        None => FALSE,
    }
}

/// Logs one blit: destination identity is the Phase-A question (window DC vs memory
/// DC), so `WindowFromDC` decides the sample and the class name is resolved for the
/// census cross-check.
/// Spike-only: a blit that *reads* a window or screen DC (s13: does the engine build the
/// translucent text box from the screen behind it, or from its own memory frame?).
/// Bounded to 300 lines.
fn note_window_read(dest: HDC, rect: [i32; 4], src: HDC, srect: [i32; 4]) {
    static N: AtomicU32 = AtomicU32::new(0);
    // SAFETY: handles come from the hooked call; plain queries.
    let (swin, kind) = unsafe { (WindowFromDC(src), GetObjectType(src)) };
    if swin.is_null() && kind != OBJ_DC as u32 {
        return; // a memory DC: the engine reading its own surfaces
    }
    if N.fetch_add(1, Ordering::Relaxed) >= 300 {
        return;
    }
    let dwin = unsafe { WindowFromDC(dest) };
    log::line(
        "winread",
        format_args!(
            "src={:#x} src_win={:#x} {:?} srect={srect:?} -> dest_win={:#x} rect={rect:?} objtype={kind}",
            src as usize,
            swin as usize,
            unsafe { class_of(swin) },
            dwin as usize
        ),
    );
}

fn observe_blt(label: &str, dest: HDC, rect: [i32; 4], src: HDC, srect: [i32; 4], rop: u32) {
    let first = BLT_SEEN.fetch_add(1, Ordering::Relaxed);
    note_window_read(dest, rect, src, srect);
    // SAFETY: handles come from the hooked call.
    let win = unsafe { WindowFromDC(dest) };
    let qualified = !win.is_null() || (rect[2] >= 256 && rect[3] >= 192);
    if !qualified {
        return;
    }
    let class = if win.is_null() {
        String::new()
    } else {
        unsafe { class_of(win) }
    };
    // Pixels are always read (that is the measurement); only the file write is
    // switchable, via PN_LOG_NODUMP.
    // SAFETY: src is a DC handle from the hooked call.
    let cap = unsafe { frame::observe_dc(src, label) };
    let (bpp, bw, bh, hash, us) = cap.as_ref().map_or((0u16, 0i32, 0i32, 0u64, 0u64), |c| {
        (c.bpp, c.w, c.h, c.hash, c.us)
    });
    if cap.is_none() && first < 12 {
        // Evidence otherwise lost: a source DC that cannot be read back (a window DC,
        // or a DDB that is not a DIB section) is not the same as "no blit".
        log::line(
            "blt",
            format_args!(
                "#{first} {label} dest={:#x} NO-CAPTURE src={:#x}",
                dest as usize, src as usize
            ),
        );
    }
    let file = cap
        .as_ref()
        .and_then(|c| c.file.clone())
        .unwrap_or_default();
    let dumped = u8::from(!file.is_empty());
    // SAFETY: src is a valid DC from the hooked call.
    let hbmp = unsafe { GetCurrentObject(src, OBJ_BITMAP as u32) } as usize;
    log::csv(format_args!(
        "{},{},{:#x},{hbmp:#x},{bpp},{bw},{bh},{},{},{},{},{},{},{},{},{rop:#x},{:#x},{:#x},{class},{hash:016x},{dumped},{file},{us}",
        log::ms(),
        label,
        src as usize,
        rect[0],
        rect[1],
        rect[2],
        rect[3],
        srect[0],
        srect[1],
        srect[2],
        srect[3],
        dest as usize,
        win as usize,
    ));
    if first < 12 {
        log::line(
            "blt",
            format_args!(
                "#{first} {label} dest={:#x} win={:#x} {class:?} rect={},{},{},{} src_bmp={hbmp:#x} us={us}",
                dest as usize, win as usize, rect[0], rect[1], rect[2], rect[3]
            ),
        );
    }
}

type CreateFontAFn = unsafe extern "system" fn(
    i32,
    i32,
    i32,
    i32,
    i32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    *const u8,
) -> windows_sys::Win32::Graphics::Gdi::HFONT;

static ORIG_CREATEFONTA: AtomicUsize = AtomicUsize::new(0);

/// The engine's text quality as a 2002 desktop drew it: grayscale antialiasing, not
/// today's `ClearType` default (session 15: its coloured subpixel fringes are
/// magnified with the frame, `work/report_img/F.png`). DEFAULT, DRAFT and PROOF quality
/// all take the system's smoothing, i.e. `ClearType`; only an explicit "no antialiasing"
/// is kept.
fn font_quality(asked: u32) -> u32 {
    const NONANTIALIASED_QUALITY: u32 = 3;
    const ANTIALIASED_QUALITY: u32 = 4;
    if asked == NONANTIALIASED_QUALITY {
        asked
    } else {
        ANTIALIASED_QUALITY
    }
}

unsafe extern "system" fn hook_create_font_a(
    h: i32,
    w: i32,
    esc: i32,
    orient: i32,
    weight: i32,
    italic: u32,
    under: u32,
    strike: u32,
    charset: u32,
    out_p: u32,
    clip_p: u32,
    quality: u32,
    pitch: u32,
    face: *const u8,
) -> windows_sys::Win32::Graphics::Gdi::HFONT {
    let q = if crate::config::enabled() {
        font_quality(quality)
    } else {
        quality
    };
    if log::spike() {
        guarded("CreateFontA", || {
            log::line(
                "createfont",
                format_args!("h={h} weight={weight} quality={quality} -> {q}"),
            );
        });
    }
    match orig::<CreateFontAFn>(&ORIG_CREATEFONTA) {
        Some(f) => unsafe {
            f(
                h, w, esc, orient, weight, italic, under, strike, charset, out_p, clip_p, q, pitch,
                face,
            )
        },
        None => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Window fights and device queries (review O4/O5, NF-3, NF-9)
// ---------------------------------------------------------------------------

type GetDeviceCapsFn = unsafe extern "system" fn(HDC, i32) -> i32;

static ORIG_GETDEVICECAPS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn hook_get_device_caps(hdc: HDC, index: i32) -> i32 {
    let r = match orig::<GetDeviceCapsFn>(&ORIG_GETDEVICECAPS) {
        Some(f) => unsafe { f(hdc, index) },
        None => 0,
    };
    if log::spike() {
        // Spike only, and capped by the spike's own 1 h window: an uncapped per-call line
        // is the one writer that could fill a log (review s14b n-1).
        guarded("GetDeviceCaps", || {
            log::line("getdevicecaps", format_args!("index={index} result={r}"));
        });
    }
    r
}

/// Flushes every counted observer; called from the census tick so the log stays
/// readable and the hooks stay allocation-free on hot paths. Sample layout is
/// `[t_ms, arg1, arg2, arg3]`, so the `first_t` field in the log is a real clock.
/// Samples are kept for the first calls only ([`crate::count`]) — a 31 kHz poll must
/// not refill them every second.
pub fn flush_counted() {
    for c in crate::vhooks::COUNTED
        .iter()
        .chain(present::COUNTERS.iter())
    {
        let (total, moved, samples) = c.take();
        if !moved && samples.is_empty() {
            continue;
        }
        if samples.is_empty() {
            log::line("count", format_args!("{} total={total}", c.label));
            continue;
        }
        let first_t = samples.first().map_or(0, |s| s[0]);
        log::line(
            "count",
            format_args!(
                "{} total={total} first_t={first_t} samples={}",
                c.label,
                samples
                    .iter()
                    .map(|s| format!("{:?}", &s[..3]))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// The hook table walked by the installer
// ---------------------------------------------------------------------------

static HOOKS: std::sync::LazyLock<Vec<HookSpec>> = std::sync::LazyLock::new(build_specs);

/// Every slot the presenter wants. Module/function names are exactly the host's import
/// names (`E02`), so the walk finds them by string. Built once, at first use: casting
/// a function item to an address is a runtime operation on this toolchain.
pub fn hooks() -> &'static [HookSpec] {
    &HOOKS
}

fn build_specs() -> Vec<HookSpec> {
    let mut specs = present_specs();
    specs.extend(crate::vhooks::window_specs());
    specs.extend(crate::vhooks::input_specs());
    specs
}

/// Present path + device queries.
fn present_specs() -> Vec<HookSpec> {
    vec![
        HookSpec::new(
            "GDI32.dll",
            "BitBlt",
            hook_bitblt as BitBltFn as usize,
            &ORIG_BITBLT,
        ),
        HookSpec::new(
            "GDI32.dll",
            "StretchBlt",
            hook_stretchblt as StretchBltFn as usize,
            &ORIG_STRETCHBLT,
        ),
        HookSpec::new(
            "GDI32.dll",
            "GetDeviceCaps",
            hook_get_device_caps as GetDeviceCapsFn as usize,
            &ORIG_GETDEVICECAPS,
        ),
        HookSpec::new(
            "GDI32.dll",
            "CreateFontA",
            hook_create_font_a as CreateFontAFn as usize,
            &ORIG_CREATEFONTA,
        ),
    ]
}

// The hook types are the imports' own signatures (see `vhooks::signatures`).
const _: [BitBltFn; 1] = [windows_sys::Win32::Graphics::Gdi::BitBlt];
const _: [StretchBltFn; 1] = [windows_sys::Win32::Graphics::Gdi::StretchBlt];
const _: [GetDeviceCapsFn; 1] = [windows_sys::Win32::Graphics::Gdi::GetDeviceCaps];
const _: [CreateFontAFn; 1] = [windows_sys::Win32::Graphics::Gdi::CreateFontA];

#[cfg(test)]
mod tests {
    #[test]
    fn cleartype_becomes_grayscale_and_explicit_choices_stay() {
        assert_eq!(super::font_quality(0), 4);
        assert_eq!(super::font_quality(5), 4);
        assert_eq!(super::font_quality(6), 4);
        assert_eq!(super::font_quality(2), 4); // PROOF: system smoothing = ClearType
        assert_eq!(super::font_quality(3), 3); // NONANTIALIASED
    }
}
