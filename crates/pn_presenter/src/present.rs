//! The present redirect (`docs/04-l2-presenter.md` §3, NF-12/NF-17).
//!
//! Every blit whose *destination DC* belongs to a managed window is re-drawn scaled by
//! the sink; everything else passes through untouched — the engine's offscreen
//! compositing (85x640x480, 544x320, 1440x720 and 1-bpp surfaces) must never be
//! transformed, so the predicate is destination identity via `WindowFromDC`, never the
//! blit size.
//!
//! Cases the redirect deliberately leaves alone, each counted so a run log shows them:
//! a non-`SRCCOPY` ROP (unknown compositing — NF-12 saw SRCCOPY only), a source window DC
//! of the same window (the engine reading its own presented pixels back), and window DCs
//! of other process windows (`DirectShow`, IME — NF-13).

// ROP/handle arithmetic.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Graphics::Gdi::{SRCCOPY, WindowFromDC};

use crate::compose;
use crate::config;
use crate::count::Counted;
use crate::geom;
use crate::log;
use crate::sink::Present;
use crate::win::{BOOL, HDC, TRUE, ms_i64};

/// Blits the engine aimed at a window the presenter manages.
static C_PRESENT: Counted = Counted::new("present");
/// The sink refused (no window DC, GDI failure) — the engine's own blit then runs.
static C_FAILED: Counted = Counted::new("present-failed");
/// Window DCs the redirect does not touch, keyed by the window's HWND.
static C_SKIP_WINDOW: Counted = Counted::new("present-skip-window");
static C_SKIP_ROP: Counted = Counted::new("present-skip-rop");
static C_SKIP_SELF: Counted = Counted::new("present-skip-self");
static LOGGED: AtomicU32 = AtomicU32::new(0);

/// Flushed with the Phase A counters (`hooks::flush_counted`).
pub static COUNTERS: [&Counted; 5] = [
    &C_PRESENT,
    &C_FAILED,
    &C_SKIP_WINDOW,
    &C_SKIP_ROP,
    &C_SKIP_SELF,
];

/// Decides what happens to one `BitBlt`/`StretchBlt` call.
///
/// `Some(result)` means the blit *was* presented (the caller must not call the engine's
/// original — the frame is on screen already); `None` means "pass it through".
pub fn intercept(
    dest: HDC,
    dst_rect: [i32; 4],
    src: HDC,
    src_rect: [i32; 4],
    rop: u32,
) -> Option<BOOL> {
    if !config::enabled() || dest.is_null() || src.is_null() {
        return None;
    }
    // SAFETY: dest comes from the hooked call.
    let hwnd = unsafe { WindowFromDC(dest) };
    if hwnd.is_null() {
        return None; // a memory DC: the engine compositing with itself (NF-17)
    }
    if !crate::window::is_frame_window(hwnd) {
        C_SKIP_WINDOW.note(|| {
            [
                ms_i64(),
                hwnd as i64,
                i64::from(dst_rect[2]),
                i64::from(dst_rect[3]),
            ]
        });
        return None;
    }
    if src == dest {
        C_SKIP_SELF.note(|| [ms_i64(), hwnd as i64, 0, 0]);
        return None;
    }
    if rop != SRCCOPY {
        C_SKIP_ROP.note(|| [ms_i64(), i64::from(rop), hwnd as i64, 0]);
        return None;
    }
    crate::window::ensure(hwnd);
    let blit = compose::Blit {
        hwnd,
        src,
        src_rect,
        dst: dst_rect,
    };
    if crate::cost::timed(|| compose::present(&blit)) {
        let present = Present {
            hwnd,
            origin: crate::window::client_logical_origin(hwnd),
            src,
            src_rect,
            logical: dst_rect,
        };
        C_PRESENT.note(|| {
            [
                ms_i64(),
                hwnd as i64,
                i64::from(dst_rect[2]),
                i64::from(dst_rect[3]),
            ]
        });
        log_first(&present);
        if log::spike() && crate::window::kind(hwnd) == Some(crate::window::Kind::Box) {
            // Dialogue is up: the engine's box source next to the *game window's* pixels
            // (the box is composed into it), sampled on the engine's thread.
            if let Some(main) = crate::window::main_window() {
                crate::census::sample_present_target(main, src);
            }
        }
        Some(TRUE)
    } else {
        C_FAILED.note(|| {
            [
                ms_i64(),
                hwnd as i64,
                i64::from(dst_rect[2]),
                i64::from(dst_rect[3]),
            ]
        });
        None
    }
}

/// The first dozen presents carry the whole mapping (logical -> physical, scale, window)
/// so a run log answers "did the transform happen at the right place" without a dump.
fn log_first(present: &Present) {
    if LOGGED.load(Ordering::Relaxed) >= 12 {
        return;
    }
    LOGGED.fetch_add(1, Ordering::Relaxed);
    let plan = geom::plan();
    let dst = geom::rect_to_physical(present.origin, plan.scale, present.logical);
    log::line(
        "present",
        format_args!(
            "hwnd={:#x} logical={},{},{},{} -> physical={},{},{},{} origin={},{} scale={:.4}",
            present.hwnd as usize,
            present.logical[0],
            present.logical[1],
            present.logical[2],
            present.logical[3],
            dst[0],
            dst[1],
            dst[2],
            dst[3],
            present.origin.0,
            present.origin.1,
            plan.scale
        ),
    );
}
