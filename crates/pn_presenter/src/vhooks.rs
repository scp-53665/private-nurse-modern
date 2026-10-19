//! The USER32 window and input hooks that keep the engine on its virtual monitor
//! (`docs/04-l2-presenter.md` §3.0, model in `virt`).
//!
//! Three rules, applied by every hook here (each one passes straight through when the
//! INI kill switch is off):
//! * **answer in V**: geometry the engine reads about its windows, the desktop, the screen
//!   and the cursor;
//! * **record in V**: geometry the engine asks for on its windows (never applied to a real
//!   window; only show/hide reaches the real ones);
//! * **refuse global side effects**: display mode, work area, cursor clip, and any window
//!   of another process. A refusal reports success and is logged (capped).

// Hook signatures mirror the imported functions parameter for parameter (x/y/w/h
// included), and Win32 flag/handle arithmetic is inherent. Justified once.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::similar_names
)]

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use windows_sys::Win32::Graphics::Gdi::PAINTSTRUCT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SW_MINIMIZE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSENDCHANGING, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, WINDOWPLACEMENT,
};

use crate::config;
use crate::count::Counted;
use crate::geom;
use crate::hooks::{caught, guarded, orig};
use crate::log;
use crate::pe::HookSpec;
use crate::virt;
use crate::win::{BOOL, DEVMODEA, FALSE, HDC, HWND, POINT, RECT, TRUE, ms_i64};
use crate::window::{self, Kind};

fn on() -> bool {
    config::enabled()
}

/// One log line per refusal, capped: a refused call in a loop must not fill the disk.
fn refused(what: &str, detail: std::fmt::Arguments) {
    static N: AtomicU32 = AtomicU32::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 300 {
        log::line("refused", format_args!("{what} {detail}"));
    }
}

fn to_rect(r: [i32; 4]) -> RECT {
    RECT {
        left: r[0],
        top: r[1],
        right: r[0] + r[2],
        bottom: r[1] + r[3],
    }
}

// ---------------------------------------------------------------------------
// Birth: the engine's windows enter V as they are created
// ---------------------------------------------------------------------------

type CreateWindowExAFn = unsafe extern "system" fn(
    u32,
    *const u8,
    *const u8,
    u32,
    i32,
    i32,
    i32,
    i32,
    HWND,
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    *const core::ffi::c_void,
) -> HWND;

static ORIG_CREATEWINDOWEXA: AtomicUsize = AtomicUsize::new(0);

/// A new window is classified on the spot (its V rect is where the engine asked for it),
/// and the text box is made invisible before it can paint on the real desktop: the engine
/// creates it visible (`WS_VISIBLE` in the create style, run vd1).
unsafe extern "system" fn hook_create_window_ex_a(
    ex: u32,
    class: *const u8,
    title: *const u8,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: HWND,
    menu: *mut core::ffi::c_void,
    inst: *mut core::ffi::c_void,
    param: *const core::ffi::c_void,
) -> HWND {
    let hwnd = match orig::<CreateWindowExAFn>(&ORIG_CREATEWINDOWEXA) {
        Some(f) => unsafe {
            f(
                ex, class, title, style, x, y, w, h, parent, menu, inst, param,
            )
        },
        None => std::ptr::null_mut(),
    };
    if on() && !hwnd.is_null() {
        guarded("CreateWindowExA", || {
            let kind = window::kind(hwnd);
            if kind == Some(Kind::Box) {
                // The create arguments are the engine's V intent; the real rect read at
                // registration can be empty (registered mid-create, run vd2: v=0,0,0,0).
                virt::set_rect(hwnd, x, y, w, h, 0);
                window::hide_box(hwnd);
            }
            if log::spike() {
                log::line(
                    "create",
                    format_args!(
                        "hwnd={:#x} {kind:?} style={style:#x} ex={ex:#x} at={x},{y},{w},{h}",
                        hwnd as usize
                    ),
                );
            }
        });
    }
    hwnd
}

// ---------------------------------------------------------------------------
// Placement: SetWindowPos / MoveWindow / ShowWindow / SetWindowLongA
// ---------------------------------------------------------------------------

type ShowWindowFn = unsafe extern "system" fn(HWND, i32) -> BOOL;
type SetWindowPosFn = unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> BOOL;
type MoveWindowFn = unsafe extern "system" fn(HWND, i32, i32, i32, i32, BOOL) -> BOOL;
type SetWindowLongAFn = unsafe extern "system" fn(HWND, i32, i32) -> i32;
type GetWindowLongAFn = unsafe extern "system" fn(HWND, i32) -> i32;

static ORIG_SHOWWINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_SETWINDOWPOS: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVEWINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_SETWINDOWLONGA: AtomicUsize = AtomicUsize::new(0);
static ORIG_GETWINDOWLONGA: AtomicUsize = AtomicUsize::new(0);

/// What happens to one engine move/size request.
enum Place {
    /// Not ours to judge (kill switch, a child control, ...): the real call runs as asked.
    Pass,
    /// Recorded in V (or refused); no real call. `bool`: the text box moved.
    Absorb(bool),
    /// A managed window shown or hidden: the real call runs with these flags only.
    Show(u32, bool),
    /// An engine dialog: placed by the presenter at this position.
    Dialog(i32, i32),
}

fn place(hwnd: HWND, r: [i32; 4], flags: u32) -> Place {
    if !on() || hwnd.is_null() {
        return Place::Pass;
    }
    if window::is_foreign(hwnd) {
        refused(
            "move",
            format_args!("foreign hwnd={:#x} flags={flags:#x}", hwnd as usize),
        );
        return Place::Absorb(false);
    }
    if let Some(kind) = window::kind(hwnd) {
        virt::set_rect(hwnd, r[0], r[1], r[2], r[3], flags);
        let is_box = kind == Kind::Box;
        if is_box {
            window::hide_box(hwnd); // idempotent: never shown on the real desktop
        }
        let show = flags & (SWP_SHOWWINDOW | SWP_HIDEWINDOW);
        if show == 0 {
            return Place::Absorb(is_box);
        }
        return Place::Show(
            // SWP_NOSENDCHANGING like every other window call of ours (window.rs): the
            // engine must never see a WINDOWPOS* about a position it did not ask for
            // (review s14b M-1/M-2).
            show | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
            is_box,
        );
    }
    match window::place_dialog(hwnd, (r[2], r[3]), flags) {
        Some((x, y)) => Place::Dialog(x, y),
        None => Place::Pass,
    }
}

unsafe extern "system" fn hook_set_window_pos(
    hwnd: HWND,
    after: HWND,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> BOOL {
    let decision = caught(|| place(hwnd, [x, y, cx, cy], flags)).unwrap_or(Place::Pass);
    let Some(f) = orig::<SetWindowPosFn>(&ORIG_SETWINDOWPOS) else {
        return FALSE;
    };
    let (r, is_box) = match decision {
        Place::Pass => (unsafe { f(hwnd, after, x, y, cx, cy, flags) }, false),
        Place::Absorb(is_box) => (TRUE, is_box),
        Place::Show(fl, is_box) => (unsafe { f(hwnd, after, 0, 0, 0, 0, fl) }, is_box),
        Place::Dialog(nx, ny) => (unsafe { f(hwnd, after, nx, ny, cx, cy, flags) }, false),
    };
    if is_box {
        guarded("SetWindowPos", || crate::compose::box_changed(hwnd));
    }
    if log::spike() {
        log::line(
            "setwindowpos",
            format_args!(
                "hwnd={:#x} after={:#x} asked={x},{y},{cx},{cy} flags={flags:#x} ret={r}",
                hwnd as usize, after as usize
            ),
        );
    }
    r
}

unsafe extern "system" fn hook_move_window(
    hwnd: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    repaint: BOOL,
) -> BOOL {
    let decision = caught(|| place(hwnd, [x, y, w, h], 0)).unwrap_or(Place::Pass);
    let Some(f) = orig::<MoveWindowFn>(&ORIG_MOVEWINDOW) else {
        return FALSE;
    };
    let (r, is_box) = match decision {
        Place::Pass | Place::Show(..) => (unsafe { f(hwnd, x, y, w, h, repaint) }, false),
        Place::Absorb(is_box) => (TRUE, is_box),
        Place::Dialog(nx, ny) => (unsafe { f(hwnd, nx, ny, w, h, repaint) }, false),
    };
    if is_box {
        guarded("MoveWindow", || crate::compose::box_changed(hwnd));
    }
    if log::spike() {
        log::line(
            "movewindow",
            format_args!("hwnd={:#x} asked={x},{y},{w},{h} ret={r}", hwnd as usize),
        );
    }
    r
}

unsafe extern "system" fn hook_show_window(hwnd: HWND, cmd: i32) -> BOOL {
    // (cmd to use, is_box, refuse)
    let (cmd, is_box, refuse) = caught(|| {
        if !on() {
            return (cmd, false, false);
        }
        if window::is_foreign(hwnd) {
            refused(
                "show",
                format_args!("foreign hwnd={:#x} cmd={cmd}", hwnd as usize),
            );
            return (cmd, false, true);
        }
        match window::kind(hwnd) {
            // O4: defocus makes the engine minimize *itself*; the user decides that now.
            // The text box too (review s14b m-3: b1d1fe9 refused it for any managed
            // window; narrowing it to Main let a minimised box animate on the desktop).
            Some(_) if cmd == SW_MINIMIZE => (cmd, false, true),
            Some(Kind::Box) => {
                window::hide_box(hwnd);
                (window::box_show_cmd(cmd), true, false)
            }
            _ => (cmd, false, false),
        }
    })
    .unwrap_or((cmd, false, false));
    if refuse {
        return TRUE; // "was visible", as a show of a visible window reports
    }
    let r = match orig::<ShowWindowFn>(&ORIG_SHOWWINDOW) {
        Some(f) => unsafe { f(hwnd, cmd) },
        None => FALSE,
    };
    if is_box {
        guarded("ShowWindow", || crate::compose::box_changed(hwnd));
    }
    if log::spike() {
        log::line(
            "showwindow",
            format_args!("hwnd={:#x} cmd={cmd} ret={r}", hwnd as usize),
        );
    }
    r
}

/// Style and exstyle of the engine's windows are V state: the real ones are the
/// presenter's (decoration, the invisible text box), the engine reads back its own.
unsafe extern "system" fn hook_set_window_long_a(hwnd: HWND, index: i32, value: i32) -> i32 {
    let v = caught(|| {
        if !on() {
            return None;
        }
        if window::is_foreign(hwnd) {
            refused(
                "setwindowlong",
                format_args!("foreign hwnd={:#x} index={index}", hwnd as usize),
            );
            return Some(0);
        }
        window::kind(hwnd)?;
        virt::set_style(hwnd, index, value)
    })
    .flatten();
    if let Some(previous) = v {
        return previous;
    }
    match orig::<SetWindowLongAFn>(&ORIG_SETWINDOWLONGA) {
        Some(f) => unsafe { f(hwnd, index, value) },
        None => 0,
    }
}

unsafe extern "system" fn hook_get_window_long_a(hwnd: HWND, index: i32) -> i32 {
    let v = caught(|| {
        if !on() {
            return None;
        }
        window::kind(hwnd)?;
        virt::style(hwnd, index)
    })
    .flatten();
    if let Some(v) = v {
        return v;
    }
    match orig::<GetWindowLongAFn>(&ORIG_GETWINDOWLONGA) {
        Some(f) => unsafe { f(hwnd, index) },
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// The screen: display mode, metrics, work area
// ---------------------------------------------------------------------------

type ChangeDisplaySettingsAFn = unsafe extern "system" fn(*const DEVMODEA, u32) -> i32;
type EnumDisplaySettingsAFn = unsafe extern "system" fn(*const u8, u32, *mut DEVMODEA) -> BOOL;
type GetSystemMetricsFn = unsafe extern "system" fn(i32) -> i32;
type SystemParametersInfoAFn =
    unsafe extern "system" fn(u32, u32, *mut core::ffi::c_void, u32) -> BOOL;

static ORIG_CHANGEDISPLAY: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENUMDISPLAY: AtomicUsize = AtomicUsize::new(0);
static ORIG_GETSYSTEMMETRICS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SPI: AtomicUsize = AtomicUsize::new(0);

/// The display mode never changes: the presenter scales the frame itself, and a mode
/// switch is what blurs an LCD panel (`m0_observed.md`).
unsafe extern "system" fn hook_change_display_settings(dm: *const DEVMODEA, flags: u32) -> i32 {
    if on() {
        // The refusal must hold even if the logging panics (AGENTS: every entry point
        // swallows panics), so the return sits outside the guarded body.
        guarded("ChangeDisplaySettingsA", || {
            let asked = if dm.is_null() {
                String::from("devmode=null")
            } else {
                // SAFETY: the engine passes a DEVMODE for its requested mode; we only read.
                let d = unsafe { &*dm };
                format!(
                    "w={} h={} bpp={}",
                    d.dmPelsWidth, d.dmPelsHeight, d.dmBitsPerPel
                )
            };
            refused("changedisplay", format_args!("{asked} flags={flags:#x}"));
        });
        return 0; // DISP_CHANGE_SUCCESSFUL
    }
    match orig::<ChangeDisplaySettingsAFn>(&ORIG_CHANGEDISPLAY) {
        Some(f) => unsafe { f(dm, flags) },
        None => 0,
    }
}

/// The *current* mode is V's 640x480 (bit depth and refresh stay real); the list of
/// modes is the real one.
unsafe extern "system" fn hook_enum_display_settings(
    name: *const u8,
    mode: u32,
    dm: *mut DEVMODEA,
) -> BOOL {
    const CURRENT: u32 = u32::MAX; // ENUM_CURRENT_SETTINGS
    const REGISTRY: u32 = u32::MAX - 1; // ENUM_REGISTRY_SETTINGS
    let r = match orig::<EnumDisplaySettingsAFn>(&ORIG_ENUMDISPLAY) {
        Some(f) => unsafe { f(name, mode, dm) },
        None => FALSE,
    };
    if on() && r != 0 && !dm.is_null() && (mode == CURRENT || mode == REGISTRY) {
        guarded("EnumDisplaySettingsA", || {
            // SAFETY: the call just filled this DEVMODE.
            unsafe {
                (*dm).dmPelsWidth = geom::FRAME.0 as u32;
                (*dm).dmPelsHeight = geom::FRAME.1 as u32;
            }
        });
    }
    r
}

/// Screen-size metrics in V. Counted, not logged per call: the engine polls
/// `SM_SWAPBUTTON` at ~31 kHz (measured 5 648 908 calls in 180 s).
unsafe extern "system" fn hook_get_system_metrics(index: i32) -> i32 {
    let (w, h) = geom::FRAME;
    let v = caught(|| {
        if on() {
            match index {
                0 | 16 | 61 | 78 => Some(w), // SM_CXSCREEN, CXFULLSCREEN, CXMAXIMIZED, CXVIRTUALSCREEN
                1 | 17 | 62 | 79 => Some(h),
                76 | 77 => Some(0), // SM_X/YVIRTUALSCREEN
                80 => Some(1),      // SM_CMONITORS
                _ => None,
            }
        } else {
            None
        }
    })
    .unwrap_or(None);
    let r = v.unwrap_or_else(
        || match orig::<GetSystemMetricsFn>(&ORIG_GETSYSTEMMETRICS) {
            Some(f) => unsafe { f(index) },
            None => 0,
        },
    );
    C_GETSYSTEMMETRICS.note(|| [ms_i64(), i64::from(index), i64::from(r), 0]);
    r
}

/// Only `SPI_GETWORKAREA` is answered (the whole V screen); every other action is
/// refused. Static scan: the EXE's only call sites are `0x41c70a` (`SPI_SETWORKAREA`,
/// NULL: it left the user's work area at 640x480, run mn1) and `0x435cb4` (GET).
unsafe extern "system" fn hook_system_parameters_info_a(
    action: u32,
    uparam: u32,
    pv: *mut core::ffi::c_void,
    winini: u32,
) -> BOOL {
    const SPI_GETWORKAREA: u32 = 0x30;
    if on() {
        // Deny by default, panic or not: the real `SPI_SETWORKAREA` is what left the
        // user's desktop at 640x480 (run mn1).
        guarded("SystemParametersInfoA", || {
            if action == SPI_GETWORKAREA && !pv.is_null() {
                // SAFETY: SPI_GETWORKAREA's pvParam is one writable RECT.
                unsafe { *pv.cast::<RECT>() = to_rect(virt::SCREEN) };
                return;
            }
            refused(
                "spi",
                format_args!("action={action:#x} uparam={uparam} winini={winini:#x}"),
            );
        });
        return TRUE;
    }
    match orig::<SystemParametersInfoAFn>(&ORIG_SPI) {
        Some(f) => unsafe { f(action, uparam, pv, winini) },
        None => FALSE,
    }
}

// ---------------------------------------------------------------------------
// Window geometry reads
// ---------------------------------------------------------------------------

type RectFn = unsafe extern "system" fn(HWND, *mut RECT) -> BOOL;
type GetWindowPlacementFn = unsafe extern "system" fn(HWND, *mut WINDOWPLACEMENT) -> BOOL;

static ORIG_GETWINDOWRECT: AtomicUsize = AtomicUsize::new(0);
static ORIG_GETCLIENTRECT: AtomicUsize = AtomicUsize::new(0);
static ORIG_GETWINDOWPLACEMENT: AtomicUsize = AtomicUsize::new(0);

/// `GetWindowRect`/`GetClientRect` of a V window (or the desktop): its V rect, or its V
/// size at (0,0) for the client.
fn v_rect_answer(hwnd: HWND, rect: *mut RECT, client: bool) -> bool {
    caught(|| {
        if !on() || rect.is_null() {
            return false;
        }
        let Some(v) = virt::rect(hwnd) else {
            return false;
        };
        let r = if client { [0, 0, v[2], v[3]] } else { v };
        // SAFETY: rect is non-null (checked) and writable per the API contract.
        unsafe { *rect = to_rect(r) };
        true
    })
    .unwrap_or(false)
}

unsafe extern "system" fn hook_get_window_rect(hwnd: HWND, rect: *mut RECT) -> BOOL {
    if v_rect_answer(hwnd, rect, false) {
        return TRUE;
    }
    match orig::<RectFn>(&ORIG_GETWINDOWRECT) {
        Some(f) => unsafe { f(hwnd, rect) },
        None => FALSE,
    }
}

unsafe extern "system" fn hook_get_client_rect(hwnd: HWND, rect: *mut RECT) -> BOOL {
    let v = v_rect_answer(hwnd, rect, true);
    C_GETCLIENTRECT.note(|| [ms_i64(), hwnd as i64, i64::from(v), 0]);
    if v {
        return TRUE;
    }
    match orig::<RectFn>(&ORIG_GETCLIENTRECT) {
        Some(f) => unsafe { f(hwnd, rect) },
        None => FALSE,
    }
}

unsafe extern "system" fn hook_get_window_placement(hwnd: HWND, wp: *mut WINDOWPLACEMENT) -> BOOL {
    let r = match orig::<GetWindowPlacementFn>(&ORIG_GETWINDOWPLACEMENT) {
        Some(f) => unsafe { f(hwnd, wp) },
        None => FALSE,
    };
    guarded("GetWindowPlacement", || {
        if on()
            && r != 0
            && !wp.is_null()
            && let Some(v) = virt::rect(hwnd)
        {
            // SAFETY: filled by the call just made.
            unsafe { (*wp).rcNormalPosition = to_rect(v) };
        }
    });
    r
}

// ---------------------------------------------------------------------------
// Points: client <-> screen, the cursor, the popup menu
// ---------------------------------------------------------------------------

type GetCursorPosFn = unsafe extern "system" fn(*mut POINT) -> BOOL;
type SetCursorPosFn = unsafe extern "system" fn(i32, i32) -> BOOL;
type GetMessagePosFn = unsafe extern "system" fn() -> u32;
type PtHwndFn = unsafe extern "system" fn(HWND, *mut POINT) -> BOOL;
type MapWindowPointsFn = unsafe extern "system" fn(HWND, HWND, *mut POINT, u32) -> i32;
type ClipCursorFn = unsafe extern "system" fn(*const RECT) -> BOOL;
type TrackPopupMenuFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    u32,
    i32,
    i32,
    i32,
    HWND,
    *const RECT,
) -> BOOL;

static ORIG_GETCURSORPOS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SETCURSORPOS: AtomicUsize = AtomicUsize::new(0);
static ORIG_GETMESSAGEPOS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SCREENTOCLIENT: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLIENTTOSCREEN: AtomicUsize = AtomicUsize::new(0);
static ORIG_MAPWINDOWPOINTS: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLIPCURSOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_TRACKPOPUPMENU: AtomicUsize = AtomicUsize::new(0);

fn cursor_v() -> bool {
    on() && caught(virt::cursor_in_v).unwrap_or(false)
}

unsafe extern "system" fn hook_get_cursor_pos(pt: *mut POINT) -> BOOL {
    let r = match orig::<GetCursorPosFn>(&ORIG_GETCURSORPOS) {
        Some(f) => unsafe { f(pt) },
        None => FALSE,
    };
    if r != 0 && !pt.is_null() && cursor_v() {
        guarded("GetCursorPos", || {
            // SAFETY: pt was just filled by the call.
            let p = unsafe { &mut *pt };
            (p.x, p.y) = virt::to_v((p.x, p.y));
        });
    }
    if !pt.is_null() {
        // SAFETY: as above.
        let p = unsafe { &*pt };
        C_GETCURSORPOS.note(|| [ms_i64(), i64::from(p.x), i64::from(p.y), i64::from(r)]);
    }
    r
}

unsafe extern "system" fn hook_set_cursor_pos(x: i32, y: i32) -> BOOL {
    let (px, py) = if cursor_v() {
        caught(|| virt::to_phys((x, y))).unwrap_or((x, y))
    } else {
        (x, y)
    };
    C_SETCURSORPOS.note(|| [ms_i64(), i64::from(x), i64::from(y), 0]);
    match orig::<SetCursorPosFn>(&ORIG_SETCURSORPOS) {
        Some(f) => unsafe { f(px, py) },
        None => FALSE,
    }
}

unsafe extern "system" fn hook_get_message_pos() -> u32 {
    let r = match orig::<GetMessagePosFn>(&ORIG_GETMESSAGEPOS) {
        Some(f) => unsafe { f() },
        None => 0,
    };
    let r = if cursor_v() {
        caught(|| virt::pack(virt::to_v(virt::unpack(r as isize))) as u32).unwrap_or(r)
    } else {
        r
    };
    C_GETMESSAGEPOS.note(|| [ms_i64(), i64::from(r & 0xffff), i64::from(r >> 16), 0]);
    r
}

/// `Client`/`ScreenToClient` for a V window: offset by its V origin (the game window's
/// is (0,0), so its two spaces coincide). Real windows (the dialogs) pass through.
fn v_offset(hwnd: HWND, pt: *mut POINT, sign: i32) -> bool {
    caught(|| {
        if !on() || pt.is_null() || window::kind(hwnd).is_none() {
            return false;
        }
        let Some(v) = virt::rect(hwnd) else {
            return false;
        };
        // SAFETY: pt is non-null (checked) and writable per the API contract.
        let p = unsafe { &mut *pt };
        p.x += sign * v[0];
        p.y += sign * v[1];
        true
    })
    .unwrap_or(false)
}

unsafe extern "system" fn hook_screen_to_client(hwnd: HWND, pt: *mut POINT) -> BOOL {
    let v = v_offset(hwnd, pt, -1);
    C_SCREENTOCLIENT.note(|| [ms_i64(), hwnd as i64, i64::from(v), 0]);
    if v {
        return TRUE;
    }
    match orig::<PtHwndFn>(&ORIG_SCREENTOCLIENT) {
        Some(f) => unsafe { f(hwnd, pt) },
        None => FALSE,
    }
}

unsafe extern "system" fn hook_client_to_screen(hwnd: HWND, pt: *mut POINT) -> BOOL {
    let v = v_offset(hwnd, pt, 1);
    C_CLIENTTOSCREEN.note(|| [ms_i64(), hwnd as i64, i64::from(v), 0]);
    if v {
        return TRUE;
    }
    match orig::<PtHwndFn>(&ORIG_CLIENTTOSCREEN) {
        Some(f) => unsafe { f(hwnd, pt) },
        None => FALSE,
    }
}

/// One `MapWindowPoints` endpoint seen from V: `Some(origin)` of a V window or the
/// screen (null / desktop); `None` for a real window.
fn endpoint(hwnd: HWND) -> Option<(i32, i32)> {
    if hwnd.is_null() {
        return Some((0, 0));
    }
    virt::rect(hwnd).map(|r| (r[0], r[1]))
}

/// A point of `hwnd`'s client space (V or real) -> V screen, and back.
fn into_v(hwnd: HWND, p: (i32, i32)) -> (i32, i32) {
    if let Some(o) = endpoint(hwnd) {
        return (p.0 + o.0, p.1 + o.1);
    }
    let c = geom::client_origin(hwnd);
    virt::to_v((p.0 + c.0, p.1 + c.1))
}

fn out_of_v(hwnd: HWND, p: (i32, i32)) -> (i32, i32) {
    if let Some(o) = endpoint(hwnd) {
        return (p.0 - o.0, p.1 - o.1);
    }
    let c = geom::client_origin(hwnd);
    let q = virt::to_phys(p);
    (q.0 - c.0, q.1 - c.1)
}

unsafe extern "system" fn hook_map_window_points(
    from: HWND,
    to: HWND,
    pts: *mut POINT,
    n: u32,
) -> i32 {
    let done = caught(|| {
        let v_side = |h: HWND| !h.is_null() && window::kind(h).is_some();
        // Only a V window makes this a V question; between real windows (a dialog's own
        // controls) the real answer is exact.
        if !on() || pts.is_null() || n == 0 || !(v_side(from) || v_side(to)) {
            return false;
        }
        for i in 0..n as usize {
            // SAFETY: pts points at n POINTs per the API contract; in-range index.
            let p = unsafe { &mut *pts.add(i) };
            (p.x, p.y) = out_of_v(to, into_v(from, (p.x, p.y)));
        }
        true
    });
    C_MAPWINDOWPOINTS.note(|| [ms_i64(), from as i64, to as i64, i64::from(n)]);
    if done == Some(true) {
        return 0; // no LTR->RTL mirroring in this process
    }
    match orig::<MapWindowPointsFn>(&ORIG_MAPWINDOWPOINTS) {
        Some(f) => unsafe { f(from, to, pts, n) },
        None => 0,
    }
}

/// Confining the user's cursor to the frame was an exclusive-fullscreen habit: refused
/// (releasing it, a null rect, passes).
unsafe extern "system" fn hook_clip_cursor(clip: *const RECT) -> BOOL {
    if on() && !clip.is_null() {
        guarded("ClipCursor", || {
            // SAFETY: documented non-null single RECT for the duration of the call.
            let r = unsafe { &*clip };
            refused(
                "clipcursor",
                format_args!("{},{},{},{}", r.left, r.top, r.right, r.bottom),
            );
        });
        return TRUE;
    }
    match orig::<ClipCursorFn>(&ORIG_CLIPCURSOR) {
        Some(f) => unsafe { f(clip) },
        None => FALSE,
    }
}

/// The right-click menu (call site `0x41fb1d`): the engine's V point, opened where the
/// user clicked.
unsafe extern "system" fn hook_track_popup_menu(
    menu: *mut core::ffi::c_void,
    flags: u32,
    x: i32,
    y: i32,
    reserved: i32,
    hwnd: HWND,
    rect: *const RECT,
) -> BOOL {
    let (px, py) = if on() {
        caught(|| virt::to_phys((x, y))).unwrap_or((x, y))
    } else {
        (x, y)
    };
    if log::spike() {
        log::line("popupmenu", format_args!("v={x},{y} -> {px},{py}"));
    }
    match orig::<TrackPopupMenuFn>(&ORIG_TRACKPOPUPMENU) {
        Some(f) => unsafe { f(menu, flags, px, py, reserved, hwnd, rect) },
        None => FALSE,
    }
}

// ---------------------------------------------------------------------------
// Paint rects of the game window
// ---------------------------------------------------------------------------

type BeginPaintFn = unsafe extern "system" fn(HWND, *mut PAINTSTRUCT) -> HDC;
type InvalidateRectFn = unsafe extern "system" fn(HWND, *const RECT, BOOL) -> BOOL;

static ORIG_BEGINPAINT: AtomicUsize = AtomicUsize::new(0);
static ORIG_INVALIDATERECT: AtomicUsize = AtomicUsize::new(0);

fn is_main(hwnd: HWND) -> bool {
    on() && window::kind(hwnd) == Some(Kind::Main)
}

/// The update region arrives in physical client pixels; the engine repaints a V rect.
/// Rounded outwards (one V pixel of margin) and kept on the V screen.
unsafe extern "system" fn hook_begin_paint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC {
    let dc = match orig::<BeginPaintFn>(&ORIG_BEGINPAINT) {
        Some(f) => unsafe { f(hwnd, ps) },
        None => std::ptr::null_mut(),
    };
    if !dc.is_null() && !ps.is_null() && is_main(hwnd) {
        guarded("BeginPaint", || {
            let plan = geom::plan();
            // SAFETY: filled by the call just made.
            let r = unsafe { &mut (*ps).rcPaint };
            let a = virt::client_to_v((r.left, r.top), &plan);
            let b = virt::client_to_v((r.right, r.bottom), &plan);
            *r = RECT {
                left: (a.0 - 1).max(0),
                top: (a.1 - 1).max(0),
                right: (b.0 + 1).min(geom::FRAME.0),
                bottom: (b.1 + 1).min(geom::FRAME.1),
            };
        });
    }
    dc
}

unsafe extern "system" fn hook_invalidate_rect(hwnd: HWND, rect: *const RECT, erase: BOOL) -> BOOL {
    let phys = if rect.is_null() || !is_main(hwnd) {
        None
    } else {
        caught(|| {
            // SAFETY: documented non-null single RECT for the duration of the call.
            let v = unsafe { &*rect };
            let plan = geom::plan();
            let p = geom::rect_to_physical(
                plan.origin,
                plan.scale,
                [
                    v.left - 1,
                    v.top - 1,
                    v.right - v.left + 2,
                    v.bottom - v.top + 2,
                ],
            );
            to_rect(p)
        })
    };
    C_INVALIDATERECT.note(|| [ms_i64(), hwnd as i64, rect as i64, i64::from(erase)]);
    let Some(f) = orig::<InvalidateRectFn>(&ORIG_INVALIDATERECT) else {
        return FALSE;
    };
    match phys {
        Some(p) => unsafe { f(hwnd, &raw const p, erase) },
        None => unsafe { f(hwnd, rect, erase) },
    }
}

// ---------------------------------------------------------------------------
// Phase A observers kept for spike runs (counted, never logged per call)
// ---------------------------------------------------------------------------

type SetCaptureFn = unsafe extern "system" fn(HWND) -> HWND;
type HwndBoolFn = unsafe extern "system" fn(HWND) -> BOOL;
type UpdateRectFn = unsafe extern "system" fn(HWND, *mut RECT, BOOL) -> BOOL;

static ORIG_SETCAPTURE: AtomicUsize = AtomicUsize::new(0);
static ORIG_ISICONIC: AtomicUsize = AtomicUsize::new(0);
static ORIG_UPDATEWINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_GETUPDATERECT: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn hook_set_capture(hwnd: HWND) -> HWND {
    let r = match orig::<SetCaptureFn>(&ORIG_SETCAPTURE) {
        Some(f) => unsafe { f(hwnd) },
        None => std::ptr::null_mut(),
    };
    guarded("SetCapture", || {
        C_SETCAPTURE.note(|| [ms_i64(), hwnd as i64, r as i64, 0]);
    });
    r
}

unsafe extern "system" fn hook_is_iconic(hwnd: HWND) -> BOOL {
    let r = match orig::<HwndBoolFn>(&ORIG_ISICONIC) {
        Some(f) => unsafe { f(hwnd) },
        None => FALSE,
    };
    guarded("IsIconic", || {
        C_ISICONIC.note(|| [ms_i64(), hwnd as i64, i64::from(r), 0]);
    });
    r
}

unsafe extern "system" fn hook_update_window(hwnd: HWND) -> BOOL {
    let r = match orig::<HwndBoolFn>(&ORIG_UPDATEWINDOW) {
        Some(f) => unsafe { f(hwnd) },
        None => FALSE,
    };
    guarded("UpdateWindow", || {
        C_UPDATEWINDOW.note(|| [ms_i64(), hwnd as i64, i64::from(r), 0]);
    });
    r
}

static C_GETUPDATERECT: Counted = Counted::new("GetUpdateRect");
static C_GETSYSTEMMETRICS: Counted = Counted::new("GetSystemMetrics");
static C_GETCURSORPOS: Counted = Counted::new("GetCursorPos");
static C_SCREENTOCLIENT: Counted = Counted::new("ScreenToClient");
static C_CLIENTTOSCREEN: Counted = Counted::new("ClientToScreen");
static C_MAPWINDOWPOINTS: Counted = Counted::new("MapWindowPoints");
static C_SETCAPTURE: Counted = Counted::new("SetCapture");
static C_GETCLIENTRECT: Counted = Counted::new("GetClientRect");
static C_ISICONIC: Counted = Counted::new("IsIconic");
static C_SETCURSORPOS: Counted = Counted::new("SetCursorPos");
static C_GETMESSAGEPOS: Counted = Counted::new("GetMessagePos");
static C_INVALIDATERECT: Counted = Counted::new("InvalidateRect");
static C_UPDATEWINDOW: Counted = Counted::new("UpdateWindow");

/// The engine's update region, counted only: `virt.rs` lists it as an un-virtualised
/// mixed-world candidate, and nothing instruments it, so "the engine never calls it on
/// the game window" was an assumption (review s14b M-5). One pass-through observer makes
/// the question answerable by the next run's counter; answering it in V is a separate,
/// evidence-gated decision.
unsafe extern "system" fn hook_get_update_rect(hwnd: HWND, rect: *mut RECT, erase: BOOL) -> BOOL {
    let r = match orig::<UpdateRectFn>(&ORIG_GETUPDATERECT) {
        Some(f) => unsafe { f(hwnd, rect, erase) },
        None => FALSE,
    };
    guarded("GetUpdateRect", || {
        C_GETUPDATERECT.note(|| {
            [
                ms_i64(),
                hwnd as i64,
                window::kind(hwnd).map_or(0, |k| match k {
                    Kind::Main => 1,
                    Kind::Box => 2,
                }),
                i64::from(r),
            ]
        });
    });
    r
}

/// Flushed by `hooks::flush_counted` on the census tick.
pub static COUNTED: [&Counted; 13] = [
    &C_GETSYSTEMMETRICS,
    &C_GETCURSORPOS,
    &C_SCREENTOCLIENT,
    &C_CLIENTTOSCREEN,
    &C_MAPWINDOWPOINTS,
    &C_SETCAPTURE,
    &C_GETCLIENTRECT,
    &C_ISICONIC,
    &C_SETCURSORPOS,
    &C_GETMESSAGEPOS,
    &C_INVALIDATERECT,
    &C_UPDATEWINDOW,
    &C_GETUPDATERECT,
];

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

fn u(func: &'static str, hook: usize, orig: &'static AtomicUsize) -> HookSpec {
    HookSpec::new("USER32.dll", func, hook, orig)
}

/// Placement, screen and geometry reads.
pub fn window_specs() -> Vec<HookSpec> {
    vec![
        u(
            "CreateWindowExA",
            hook_create_window_ex_a as CreateWindowExAFn as usize,
            &ORIG_CREATEWINDOWEXA,
        ),
        u(
            "ShowWindow",
            hook_show_window as ShowWindowFn as usize,
            &ORIG_SHOWWINDOW,
        ),
        u(
            "SetWindowPos",
            hook_set_window_pos as SetWindowPosFn as usize,
            &ORIG_SETWINDOWPOS,
        ),
        u(
            "MoveWindow",
            hook_move_window as MoveWindowFn as usize,
            &ORIG_MOVEWINDOW,
        ),
        u(
            "SetWindowLongA",
            hook_set_window_long_a as SetWindowLongAFn as usize,
            &ORIG_SETWINDOWLONGA,
        ),
        u(
            "GetWindowLongA",
            hook_get_window_long_a as GetWindowLongAFn as usize,
            &ORIG_GETWINDOWLONGA,
        ),
        u(
            "ChangeDisplaySettingsA",
            hook_change_display_settings as ChangeDisplaySettingsAFn as usize,
            &ORIG_CHANGEDISPLAY,
        ),
        u(
            "EnumDisplaySettingsA",
            hook_enum_display_settings as EnumDisplaySettingsAFn as usize,
            &ORIG_ENUMDISPLAY,
        ),
        u(
            "GetSystemMetrics",
            hook_get_system_metrics as GetSystemMetricsFn as usize,
            &ORIG_GETSYSTEMMETRICS,
        ),
        u(
            "SystemParametersInfoA",
            hook_system_parameters_info_a as SystemParametersInfoAFn as usize,
            &ORIG_SPI,
        ),
        u(
            "GetWindowRect",
            hook_get_window_rect as RectFn as usize,
            &ORIG_GETWINDOWRECT,
        ),
        u(
            "GetClientRect",
            hook_get_client_rect as RectFn as usize,
            &ORIG_GETCLIENTRECT,
        ),
        u(
            "GetWindowPlacement",
            hook_get_window_placement as GetWindowPlacementFn as usize,
            &ORIG_GETWINDOWPLACEMENT,
        ),
    ]
}

/// Points, cursor, menu, paint rects and the kept observers.
pub fn input_specs() -> Vec<HookSpec> {
    vec![
        u(
            "GetCursorPos",
            hook_get_cursor_pos as GetCursorPosFn as usize,
            &ORIG_GETCURSORPOS,
        ),
        u(
            "SetCursorPos",
            hook_set_cursor_pos as SetCursorPosFn as usize,
            &ORIG_SETCURSORPOS,
        ),
        u(
            "GetMessagePos",
            hook_get_message_pos as GetMessagePosFn as usize,
            &ORIG_GETMESSAGEPOS,
        ),
        u(
            "ScreenToClient",
            hook_screen_to_client as PtHwndFn as usize,
            &ORIG_SCREENTOCLIENT,
        ),
        u(
            "ClientToScreen",
            hook_client_to_screen as PtHwndFn as usize,
            &ORIG_CLIENTTOSCREEN,
        ),
        u(
            "MapWindowPoints",
            hook_map_window_points as MapWindowPointsFn as usize,
            &ORIG_MAPWINDOWPOINTS,
        ),
        u(
            "ClipCursor",
            hook_clip_cursor as ClipCursorFn as usize,
            &ORIG_CLIPCURSOR,
        ),
        u(
            "TrackPopupMenu",
            hook_track_popup_menu as TrackPopupMenuFn as usize,
            &ORIG_TRACKPOPUPMENU,
        ),
        u(
            "BeginPaint",
            hook_begin_paint as BeginPaintFn as usize,
            &ORIG_BEGINPAINT,
        ),
        u(
            "InvalidateRect",
            hook_invalidate_rect as InvalidateRectFn as usize,
            &ORIG_INVALIDATERECT,
        ),
        u(
            "SetCapture",
            hook_set_capture as SetCaptureFn as usize,
            &ORIG_SETCAPTURE,
        ),
        u(
            "IsIconic",
            hook_is_iconic as HwndBoolFn as usize,
            &ORIG_ISICONIC,
        ),
        u(
            "UpdateWindow",
            hook_update_window as HwndBoolFn as usize,
            &ORIG_UPDATEWINDOW,
        ),
        u(
            "GetUpdateRect",
            hook_get_update_rect as UpdateRectFn as usize,
            &ORIG_GETUPDATERECT,
        ),
    ]
}

// Every hook has the import's own signature (`windows-sys`): with stdcall a wrong arity
// is a stack imbalance in the engine (session 15: `GetUpdateRect` was declared with two
// arguments of three and crashed the game on its first call). Compile-time, every build.
mod signatures {
    use super::{
        BeginPaintFn, ChangeDisplaySettingsAFn, ClipCursorFn, CreateWindowExAFn,
        EnumDisplaySettingsAFn, GetCursorPosFn, GetMessagePosFn, GetSystemMetricsFn,
        GetWindowLongAFn, GetWindowPlacementFn, HwndBoolFn, InvalidateRectFn, MapWindowPointsFn,
        MoveWindowFn, PtHwndFn, RectFn, SetCaptureFn, SetCursorPosFn, SetWindowLongAFn,
        SetWindowPosFn, ShowWindowFn, SystemParametersInfoAFn, TrackPopupMenuFn, UpdateRectFn,
    };
    use windows_sys::Win32::Graphics::Gdi as G;
    use windows_sys::Win32::UI::WindowsAndMessaging as W;

    const _: [CreateWindowExAFn; 1] = [W::CreateWindowExA];
    const _: [ShowWindowFn; 1] = [W::ShowWindow];
    const _: [SetWindowPosFn; 1] = [W::SetWindowPos];
    const _: [MoveWindowFn; 1] = [W::MoveWindow];
    const _: [SetWindowLongAFn; 1] = [W::SetWindowLongA];
    const _: [GetWindowLongAFn; 1] = [W::GetWindowLongA];
    const _: [ChangeDisplaySettingsAFn; 1] = [G::ChangeDisplaySettingsA];
    const _: [EnumDisplaySettingsAFn; 1] = [G::EnumDisplaySettingsA];
    const _: [GetSystemMetricsFn; 1] = [W::GetSystemMetrics];
    const _: [SystemParametersInfoAFn; 1] = [W::SystemParametersInfoA];
    const _: [RectFn; 2] = [W::GetWindowRect, W::GetClientRect];
    const _: [GetWindowPlacementFn; 1] = [W::GetWindowPlacement];
    const _: [GetCursorPosFn; 1] = [W::GetCursorPos];
    const _: [SetCursorPosFn; 1] = [W::SetCursorPos];
    const _: [GetMessagePosFn; 1] = [W::GetMessagePos];
    const _: [PtHwndFn; 2] = [G::ScreenToClient, G::ClientToScreen];
    const _: [MapWindowPointsFn; 1] = [G::MapWindowPoints];
    const _: [ClipCursorFn; 1] = [W::ClipCursor];
    const _: [TrackPopupMenuFn; 1] = [W::TrackPopupMenu];
    const _: [BeginPaintFn; 1] = [G::BeginPaint];
    const _: [InvalidateRectFn; 1] = [G::InvalidateRect];
    const _: [SetCaptureFn; 1] = [windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture];
    const _: [HwndBoolFn; 2] = [W::IsIconic, G::UpdateWindow];
    const _: [UpdateRectFn; 1] = [G::GetUpdateRect];
}
