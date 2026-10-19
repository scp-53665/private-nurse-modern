//! Coordinate spaces (review NF-8) and the presenter's layout math.
//!
//! *Logical* space is the engine's world: a 640x480 client whose origin is (0,0).
//! *Physical* space is the real window: the logical frame is scaled by [`Plan::scale`]
//! and placed at [`Plan::origin`] inside the client (centred — black bars — only when
//! the aspect does not match, i.e. borderless fullscreen on a non-4:3 desktop).
//!
//! The math is pure integer-in/integer-out plus one `f64` scale, so the layout is
//! unit-tested ([`tests`]) without a window, a desktop or the game. The Win32 lookups
//! (`GetSystemMetrics`, `ClientToScreen`) sit in the thin impure layer at the bottom.

// Layout arithmetic widens/narrows i32<->f64 and packs rects; inherent to the job.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::sync::Mutex;

use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics;

use crate::config::{self, Mode, Scale};
use crate::win::{HWND, POINT};

/// The engine's logical client size (`E02`: every `GetClientRect` returns 640x480).
pub const FRAME: (i32, i32) = (640, 480);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Physical pixels per logical pixel.
    pub scale: f64,
    /// Physical client size the game window should have.
    pub client: (i32, i32),
    /// Offset of logical (0,0) inside the client (bars); (0,0) windowed.
    pub origin: (i32, i32),
}

/// No presenter: the engine's own geometry, byte for byte.
pub const IDENTITY: Plan = Plan {
    scale: 1.0,
    client: FRAME,
    origin: (0, 0),
};

/// The plan in force. Set at load and again when the presenter switches between windowed
/// and borderless fullscreen (the engine's *Window mode* command, `docs/04` §3.0).
static PLAN: Mutex<Plan> = Mutex::new(IDENTITY);

/// Largest integer scale the desktop holds (the `auto` rule: 2x at >=1080p, 1x at
/// <=720p), capped at 4x.
pub fn auto_preset(desk: (i32, i32)) -> u32 {
    u32::try_from((fit_scale(desk).floor() as i64).clamp(1, 4)).unwrap_or(1)
}

fn fit_scale(area: (i32, i32)) -> f64 {
    f64::min(
        f64::from(area.0) / f64::from(FRAME.0),
        f64::from(area.1) / f64::from(FRAME.1),
    )
}

pub fn size_for(scale: f64) -> (i32, i32) {
    (
        (f64::from(FRAME.0) * scale).round() as i32,
        (f64::from(FRAME.1) * scale).round() as i32,
    )
}

/// Windowed plan: the client is exactly the frame, integer scales unless `Fit`.
///
/// `work` is the desktop work area and `decor_h` the height the window decoration adds:
/// every scale must leave room for the caption and the taskbar.
pub fn windowed(work: (i32, i32), decor_h: i32, scale: Scale) -> Plan {
    // Review s12/s13 P1-2: the cap holds the *outer* window (caption + taskbar excluded),
    // not the raw desktop: 3x on 2560x1440 put the bottom of the client off screen.
    let cap = auto_preset((work.0, work.1 - decor_h));
    let s = match scale {
        Scale::Auto => f64::from(cap),
        Scale::Preset(n) => f64::from(n.clamp(1, cap)),
        Scale::Fit => f64::min(
            f64::from(work.0) / f64::from(FRAME.0),
            f64::from(work.1 - decor_h) / f64::from(FRAME.1),
        )
        .max(0.25),
    };
    Plan {
        scale: s,
        client: size_for(s),
        origin: (0, 0),
    }
}

/// Borderless fullscreen: the client is the whole area, the frame is aspect-fitted.
pub fn fullscreen(area: (i32, i32)) -> Plan {
    let s = fit_scale(area);
    let (w, h) = size_for(s);
    Plan {
        scale: s,
        client: area,
        origin: ((area.0 - w) / 2, (area.1 - h) / 2),
    }
}

/// Logical point -> physical point, given the frame origin on screen.
pub fn to_physical(origin: (i32, i32), scale: f64, p: (i32, i32)) -> (i32, i32) {
    (
        origin.0 + (f64::from(p.0) * scale).round() as i32,
        origin.1 + (f64::from(p.1) * scale).round() as i32,
    )
}

/// Physical point -> logical point (the inverse of [`to_physical`]).
pub fn to_logical(origin: (i32, i32), scale: f64, p: (i32, i32)) -> (i32, i32) {
    (
        (f64::from(p.0 - origin.0) / scale).round() as i32,
        (f64::from(p.1 - origin.1) / scale).round() as i32,
    )
}

/// `[x, y, w, h]` logical -> physical.
pub fn rect_to_physical(origin: (i32, i32), scale: f64, rect: [i32; 4]) -> [i32; 4] {
    // Edges are rounded, not the size: `round(x*s)+round(w*s)` can miss `round((x+w)*s)` by
    // a pixel and leave a stale column (review s12/s13 P1-3).
    let (x, y) = to_physical(origin, scale, (rect[0], rect[1]));
    let (r, b) = to_physical(origin, scale, (rect[0] + rect[2], rect[1] + rect[3]));
    [x, y, r - x, b - y]
}

/// Largest frame (destination pixels) the `auto` filter draws smooth: a 1080p-class frame
/// (1440×1080 = 1 555 200 px, plus margin). User decision A1 (session 13, `s13_deferred.md`):
/// GDI's HALFTONE costs ~10 ms per full frame at 1440×1080 but ~19 ms at 1920×1440 and
/// ~37 ms at 4K (`tools/gdi_bench.py`), past the ~15 ms trigger; nearest stays ≤4 ms at 4K.
const SMOOTH_MAX_PIXELS: f64 = 1_600_000.0;

/// `auto`: nearest at integer scales (exact pixels), smooth for fractional scales up to a
/// 1080p-class frame, nearest above that. An explicit `smooth`/`nearest` always wins.
fn nearest(scale: config::Filter, s: f64) -> config::Filter {
    match scale {
        config::Filter::Auto => {
            let frame_pixels = (f64::from(FRAME.0) * s) * (f64::from(FRAME.1) * s);
            if (s - s.round()).abs() < 1e-6 || frame_pixels > SMOOTH_MAX_PIXELS {
                config::Filter::Nearest
            } else {
                config::Filter::Smooth
            }
        }
        f => f,
    }
}

/// The filter in force for this plan (used by the GDI sink).
pub fn filter_for(plan: &Plan, cfg: config::Filter) -> config::Filter {
    nearest(cfg, plan.scale)
}

// ---------------------------------------------------------------------------
// Live lookups (Win32). Everything below needs the game running.
// ---------------------------------------------------------------------------

/// Computes and stores the first plan, from the INI. Called once, from the presenter
/// thread, before the hooks are installed.
pub fn init() {
    let cfg = config::current();
    let desk = desktop();
    let plan = if cfg.enabled {
        compute(cfg.mode, desk, work_area(desk))
    } else {
        IDENTITY
    };
    store(plan, cfg.mode, desk);
}

/// The plan for `mode` on a monitor of size `monitor` whose work area is `work`.
pub fn compute(mode: Mode, monitor: (i32, i32), work: (i32, i32)) -> Plan {
    match mode {
        Mode::Fullscreen => fullscreen(monitor),
        Mode::Windowed => windowed(work, crate::window::decorated(), config::current().scale),
    }
}

/// Replaces the plan in force (and logs it: every run log names its layouts).
pub fn store(plan: Plan, mode: Mode, desk: (i32, i32)) {
    let cfg = config::current();
    let physical = physical_desktop();
    crate::log::line(
        "plan",
        format_args!(
            "enabled={} mode={mode:?} scale={:?} desk={}x{} physical={}x{} scale={:.6} client={}x{} origin={},{} filter={:?}",
            cfg.enabled,
            cfg.scale,
            desk.0,
            desk.1,
            physical.0,
            physical.1,
            plan.scale,
            plan.client.0,
            plan.client.1,
            plan.origin.0,
            plan.origin.1,
            filter_for(&plan, cfg.filter)
        ),
    );
    *PLAN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = plan;
}

/// Physical desktop size. It differs from `GetSystemMetrics` when the process is
/// DPI-virtualised — measured on the dev box: a virtual 1280x720 desktop for a 1920x1080
/// panel at 150 %. Logged, never used for sizing: an unaware process's window coordinates
/// live in *virtual* space (a virtual-sized window is what covers the screen); the crisp
/// path is `dpi_aware=1` (A2, `docs/04` §3 DPI).
fn physical_desktop() -> (i32, i32) {
    use windows_sys::Win32::Graphics::Gdi::{DESKTOPHORZRES, DESKTOPVERTRES, GetDeviceCaps};
    // SAFETY: GetDC(NULL) is the screen DC; both calls are read-only metrics.
    unsafe {
        let dc = crate::win::GetDC(std::ptr::null_mut());
        if dc.is_null() {
            return desktop();
        }
        let size = (
            GetDeviceCaps(dc, DESKTOPHORZRES.cast_signed()),
            GetDeviceCaps(dc, DESKTOPVERTRES.cast_signed()),
        );
        crate::win::ReleaseDC(std::ptr::null_mut(), dc);
        if size.0 <= 0 || size.1 <= 0 {
            desktop()
        } else {
            size
        }
    }
}

pub fn plan() -> Plan {
    *PLAN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn scale() -> f64 {
    plan().scale
}

pub fn desktop() -> (i32, i32) {
    // SAFETY: documented metric indices, no pointers.
    unsafe {
        (
            GetSystemMetrics(0), // SM_CXSCREEN
            GetSystemMetrics(1), // SM_CYSCREEN
        )
    }
}

/// Desktop work area, so `Fit`/fullscreen do not hide the frame under the taskbar.
pub fn work_area(desk: (i32, i32)) -> (i32, i32) {
    let mut r = crate::win::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: SPI_GETWORKAREA (0x30) writes one RECT into a valid out-parameter.
    let ok = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            0x30,
            0,
            (&raw mut r).cast(),
            0,
        )
    };
    if ok == 0 || r.right <= r.left || r.bottom <= r.top {
        return desk;
    }
    (r.right - r.left, r.bottom - r.top)
}

/// Physical screen position of `hwnd`'s client origin.
///
/// This is *our* call of `ClientToScreen`, not the engine's: `pe::install` patches the
/// host image's IAT only, so our own slot still reaches USER32 (the virtualising hook
/// would otherwise fold this into logical space).
pub fn client_origin(hwnd: HWND) -> (i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    // SAFETY: hwnd is a live window of this process; pt is a valid POINT.
    unsafe { ClientToScreen(hwnd, &raw mut pt) };
    (pt.x, pt.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_filter_is_smooth_up_to_a_1080p_class_frame_only() {
        use config::Filter::{Auto, Nearest, Smooth};
        // fractional, measured layouts (s13): fit 1.44x, DPI-aware windowed 2.17x and
        // fullscreen 2.25x (a 1440x1080 frame) stay smooth
        for s in [1.439_583, 2.166_667, 2.25] {
            assert_eq!(nearest(Auto, s), Smooth, "scale {s}");
        }
        // above the 1080p class: 1600x1200 (2.5x), 1440p (3.0x is integer anyway),
        // 4K 4:3 (4.5x) -> nearest; integer scales -> nearest
        for s in [2.3, 2.5, 3.375, 4.5, 1.0, 2.0, 3.0] {
            assert_eq!(nearest(Auto, s), Nearest, "scale {s}");
        }
        // an explicit choice always wins
        assert_eq!(nearest(Smooth, 4.5), Smooth);
        assert_eq!(nearest(Nearest, 1.44), Nearest);
    }

    #[test]
    fn auto_follows_the_desktop() {
        assert_eq!(auto_preset((1280, 720)), 1);
        assert_eq!(auto_preset((1920, 1080)), 2);
        assert_eq!(auto_preset((3840, 2160)), 4);
        assert_eq!(auto_preset((640, 480)), 1);
    }

    #[test]
    fn windowed_auto_and_preset_are_integer_and_clamped() {
        let p = windowed((1920, 1040), 39, Scale::Auto);
        assert!((p.scale - 2.0).abs() < 1e-9);
        assert_eq!(p.client, (1280, 960));
        assert_eq!(p.origin, (0, 0));
        // preset 4 on a 720p desktop clamps to what fits
        let p = windowed((1280, 680), 39, Scale::Preset(4));
        assert!((p.scale - 1.0).abs() < 1e-9);
        assert_eq!(p.client, FRAME);
    }

    #[test]
    fn windowed_fit_leaves_room_for_decoration() {
        let p = windowed((1280, 680), 40, Scale::Fit);
        assert!((p.scale - 1.333_333).abs() < 1e-4, "{p:?}");
        assert!(p.client.1 <= 680 - 40 + 1);
    }

    #[test]
    fn fullscreen_is_aspect_fit_with_bars() {
        let p = fullscreen((1280, 720));
        assert!((p.scale - 1.5).abs() < 1e-9);
        assert_eq!(p.client, (1280, 720));
        assert_eq!(p.origin, (160, 0));
        // the frame plus both bars is exactly the client
        assert_eq!(p.origin.0 * 2 + size_for(p.scale).0, 1280);
    }

    #[test]
    fn round_trip_point_and_rect() {
        let (o, s) = ((160, 0), 1.5);
        for p in [(0, 0), (1, 1), (125, 60), (639, 479)] {
            assert_eq!(to_logical(o, s, to_physical(o, s, p)), p);
        }
        assert_eq!(rect_to_physical(o, s, [0, 0, 640, 480]), [160, 0, 960, 720]);
        assert_eq!(
            rect_to_physical((0, 0), 2.0, [4, 376, 632, 104]),
            [8, 752, 1264, 208]
        );
    }

    #[test]
    fn rect_edges_land_where_the_neighbour_starts() {
        // adjacent logical rects must tile with no gap or overlap at a fractional scale
        let s = 1.439_583;
        for x in 0..600 {
            for w in [8, 12, 22] {
                let a = rect_to_physical((3, 26), s, [x, 10, w, 4]);
                let b = rect_to_physical((3, 26), s, [x + w, 10, w, 4]);
                assert_eq!(a[0] + a[2], b[0], "x={x} w={w}");
            }
        }
    }

    #[test]
    fn windowed_auto_holds_the_outer_window_on_1440p() {
        let p = windowed((2560, 1400), 39, Scale::Auto);
        assert!((p.scale - 2.0).abs() < 1e-9, "{p:?}");
        assert!(p.client.1 + 39 <= 1400);
        // 3x needs 1440 client + 39 decoration
        assert!((windowed((2560, 1479), 39, Scale::Auto).scale - 3.0).abs() < 1e-9);
        assert!((windowed((2560, 1478), 39, Scale::Auto).scale - 2.0).abs() < 1e-9);
    }

    #[test]
    fn identity_plan_is_a_no_op() {
        assert_eq!(to_physical((0, 0), IDENTITY.scale, (100, 50)), (100, 50));
        assert_eq!(
            rect_to_physical((0, 0), IDENTITY.scale, [1, 2, 3, 4]),
            [1, 2, 3, 4]
        );
    }
}
