//! The real windows (`docs/04-l2-presenter.md` §3.0): the presenter's host window and the
//! engine's other windows as they exist on the real desktop.
//!
//! The engine never places a real window: its requests are recorded in V (`virt`). This
//! module is the presenter's own policy:
//! * the game window is the **host**: windowed (decorated, centred on the work area, never
//!   topmost) or borderless fullscreen (the monitor rect, not topmost), switchable at
//!   runtime by the engine's *Window mode* command ([`toggle_mode`]);
//! * the text box is composed into the host (`compose`); its own window stays alive for
//!   the engine but is invisible and click-through ([`hide_box`]);
//! * the engine's dialogs are real windows, centred over the frame ([`place_dialog`]);
//! * windows of other processes (the taskbar) are never touched ([`is_foreign`]).
//!
//! Everything here runs on the engine's own thread (the present hook, a window hook, or
//! the subclass proc): our window calls carry `SWP_NOSENDCHANGING`, so a cross-thread
//! `WM_WINDOWPOSCHANGING` can never block a presenter call (the NF-18 lesson).

// Window/handle arithmetic, style bits and rect packing.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetWindowLongW, GetWindowRect,
    HWND_NOTOPMOST, IsWindow, IsWindowVisible, LWA_ALPHA, SW_RESTORE, SW_SHOW, SW_SHOWNA,
    SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowLongW,
    SetWindowPos, WS_CAPTION, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
};

use crate::config::{self, Mode};
use crate::geom;
use crate::win::{HWND, RECT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `ScriptMagic:Private Nurse` — the engine's present target, the presenter's host.
    Main,
    /// The dialogue text box (NF-12): an MFC popup (`Afx:…` class, `WS_POPUP`), created
    /// per dialogue.
    Box,
}

/// Style the presenter gives the host in windowed mode. No `WS_THICKFRAME`: resizable
/// windowed mode is the tentative item of `04` §6 and ships as fixed presets.
pub const DECORATED: u32 = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

struct Rec {
    hwnd: usize,
    kind: Kind,
    /// Host set up / box hidden once, on the engine's thread ([`ensure`]).
    done: bool,
}

static RECS: Mutex<Vec<Rec>> = Mutex::new(Vec::new());

/// The live presenter mode: 0 = not decided yet (the INI's), 1 windowed, 2 fullscreen.
static MODE: AtomicU8 = AtomicU8::new(0);

pub fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Windowed,
        2 => Mode::Fullscreen,
        _ => config::current().mode,
    }
}

fn set_mode(m: Mode) {
    MODE.store(if m == Mode::Fullscreen { 2 } else { 1 }, Ordering::Relaxed);
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: buf is a writable buffer of the size passed; sends no message.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        String::new()
    } else {
        crate::win::narrow(&buf)
    }
}

/// Classifies a window by class name (`E02`/`NF-12`); `None` for anything else, which is
/// what keeps the `DirectShow` windows (NF-13), the IME windows and the dialogs out of V.
fn classify(hwnd: HWND) -> Option<Kind> {
    let class = class_name(hwnd);
    if class.starts_with("ScriptMagic") {
        return Some(Kind::Main);
    }
    if class.starts_with("Afx") {
        // Structural, not a side effect: the engine creates the box as an `Afx:` popup
        // (`style=0x94000000 ex=0`, run vd1). Its `WS_EX_TOPMOST`, the session-13 rule,
        // only arrives through a `SetWindowPos` that V now absorbs.
        // SAFETY: style read for a window of this process.
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
        if style & WS_POPUP != 0 && style & WS_CHILD == 0 {
            return Some(Kind::Box);
        }
    }
    None
}

fn is_ours(hwnd: HWND) -> bool {
    let mut pid = 0u32;
    // SAFETY: valid u32 out-parameter.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, &raw mut pid)
    };
    pid == unsafe { GetCurrentProcessId() }
}

/// A window of another process (the shell's taskbar: the engine hides it on focus,
/// review s12/s13 P2-1). The engine never gets to move, show or hide one.
pub fn is_foreign(hwnd: HWND) -> bool {
    !hwnd.is_null() && !is_ours(hwnd)
}

/// Cached classification: `ScreenToClient` runs at ~10 kHz (R2), a class lookup per call
/// would be a presenter tax on the engine's poll loop.
pub fn kind(hwnd: HWND) -> Option<Kind> {
    if hwnd.is_null() {
        return None;
    }
    {
        let recs = RECS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(r) = recs.iter().find(|r| r.hwnd == hwnd as usize) {
            return Some(r.kind);
        }
    }
    let k = if is_ours(hwnd) { classify(hwnd) } else { None };
    if let Some(kind) = k {
        register(hwnd, kind);
    }
    k
}

pub fn is_frame_window(hwnd: HWND) -> bool {
    kind(hwnd).is_some()
}

fn register(hwnd: HWND, kind: Kind) {
    {
        let mut recs = RECS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if recs.iter().any(|r| r.hwnd == hwnd as usize) {
            return;
        }
        if recs.len() >= 32 {
            recs.retain(|r| r.kind == Kind::Main); // the box is re-created per dialogue
        }
        recs.push(Rec {
            hwnd: hwnd as usize,
            kind,
            done: false,
        });
    }
    crate::log::line(
        "window",
        format_args!("managed hwnd={:#x} kind={kind:?}", hwnd as usize),
    );
    crate::virt::adopt(hwnd, kind);
}

/// The registered game window, if the census has seen it yet.
pub fn main_window() -> Option<HWND> {
    let recs = RECS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    recs.iter()
        .find(|r| r.kind == Kind::Main)
        .map(|r| r.hwnd as HWND)
}

/// The engine's text box, if one is alive and shown (the newest: the box is re-created per
/// dialogue and old handles linger in the registry).
pub fn visible_box() -> Option<HWND> {
    let boxes: Vec<usize> = RECS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|r| r.kind == Kind::Box)
        .map(|r| r.hwnd)
        .collect();
    boxes.into_iter().rev().map(|h| h as HWND).find(|&h| {
        // SAFETY: plain queries; a stale handle reads as "not a window".
        unsafe { IsWindow(h) != 0 && IsWindowVisible(h) != 0 }
    })
}

/// Height the decoration adds to a client, used by the `Fit` scale (`geom::windowed`).
pub fn decorated() -> i32 {
    outer_size((0, 0)).1
}

/// Outer size for a wanted client size under [`DECORATED`].
pub fn outer_size(client: (i32, i32)) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: client.0,
        bottom: client.1,
    };
    // SAFETY: r is a valid RECT; the style is ours.
    unsafe { AdjustWindowRectEx(&raw mut r, DECORATED, 0, 0) };
    (r.right - r.left, r.bottom - r.top)
}

/// The frame's (0,0) in the host's **client** coordinates (the space of `GetDC(host)`,
/// where the sink draws): the plan's letterbox offset. The text box is never drawn
/// into its own window, so it has none.
pub fn client_logical_origin(hwnd: HWND) -> (i32, i32) {
    if kind(hwnd) == Some(Kind::Main) {
        geom::plan().origin
    } else {
        (0, 0)
    }
}

// ---------------------------------------------------------------------------
// First sight: one setup per window, on the engine's thread
// ---------------------------------------------------------------------------

/// Sets a managed window up once (idempotent). Called from the present hook — the engine's
/// own thread, guaranteed to be running whenever there is a frame to present.
pub fn ensure(hwnd: HWND) {
    if !config::enabled() {
        return;
    }
    let Some(kind) = kind(hwnd) else {
        return;
    };
    {
        let mut recs = RECS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(r) = recs.iter_mut().find(|r| r.hwnd == hwnd as usize) else {
            return;
        };
        if r.done {
            return;
        }
        r.done = true;
    }
    match kind {
        Kind::Main => apply_host(hwnd),
        Kind::Box => hide_box(hwnd),
    }
}

/// Lays the host out for the current [`mode`] on the monitor it is on: re-plans, restyles,
/// places (windowed: centred on the work area), and re-presents the whole frame.
fn apply_host(hwnd: HWND) {
    let (mon, work) = monitor_rects(hwnd);
    let mode = mode();
    let plan = geom::compute(mode, (mon[2], mon[3]), (work[2], work[3]));
    geom::store(plan, mode, (mon[2], mon[3]));
    let rect = match mode {
        Mode::Fullscreen => {
            set_style(hwnd, borderless_style(current_style(hwnd)));
            mon
        }
        Mode::Windowed => {
            set_style(hwnd, decorated_style(current_style(hwnd)));
            let (w, h) = outer_size(plan.client);
            let (x, y) = crate::virt::centred(work, (w, h), work);
            [x, y, w, h]
        }
    };
    set_pos(hwnd, rect);
    crate::log::line(
        "window",
        format_args!("host {mode:?} rect={rect:?} client={:?}", plan.client),
    );
    crate::compose::refresh();
}

/// The engine's *Window mode* command: the presenter switches windowed <-> borderless
/// fullscreen; the engine stays in its V fullscreen and never knows.
pub fn toggle_mode() {
    let next = match mode() {
        Mode::Windowed => Mode::Fullscreen,
        Mode::Fullscreen => Mode::Windowed,
    };
    set_mode(next);
    if let Some(main) = main_window() {
        apply_host(main);
    }
}

/// The engine's *Default window position* command: re-centre the windowed host.
pub fn recentre() {
    if mode() == Mode::Windowed
        && let Some(main) = main_window()
    {
        apply_host(main);
    }
}

/// The text box is composed into the host (`compose`), so its own window must never show
/// or take input: fully transparent (layered, alpha 0), click-through (the host underneath
/// gets the clicks the engine's `IG` op waits for), no taskbar or alt-tab entry, never
/// activated. Idempotent; called before the engine first shows it.
pub fn hide_box(hwnd: HWND) {
    const WANT: u32 = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    // SAFETY: style read/write on a window of this process, on its own thread.
    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 };
    if ex & WANT == WANT {
        return;
    }
    unsafe { SetWindowLongW(hwnd, GWL_EXSTYLE, (ex | WANT) as i32) };
    // SAFETY: the window is layered now; alpha 0 makes it invisible but alive.
    unsafe { SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA) };
    crate::log::line(
        "window",
        format_args!(
            "box hidden hwnd={:#x} ex={ex:#x} -> {:#x}",
            hwnd as usize,
            ex | WANT
        ),
    );
}

/// `ShowWindow` for the text box: shown, but never activated — activation would move
/// keyboard focus and the active caption away from the host.
pub fn box_show_cmd(cmd: i32) -> i32 {
    match cmd {
        SW_SHOWNORMAL | SW_RESTORE => SW_SHOWNOACTIVATE,
        SW_SHOW => SW_SHOWNA,
        other => other,
    }
}

/// One of the engine's dialogs (or message boxes): a real top-level window of ours that is
/// not in V and not a menu.
fn is_dialog(hwnd: HWND) -> bool {
    if hwnd.is_null() || !is_ours(hwnd) || kind(hwnd).is_some() {
        return false;
    }
    // SAFETY: style read for a window of this process.
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    style & WS_CHILD == 0 && class_name(hwnd) != "#32768"
}

/// An engine move of one of its dialogs: the position it computed mixes V and real
/// numbers (MFC `CenterWindow` on a V owner), so the presenter centres the dialog over the
/// frame instead, inside the monitor's work area. `None`: not a dialog move.
pub fn place_dialog(hwnd: HWND, size: (i32, i32), flags: u32) -> Option<(i32, i32)> {
    if flags & SWP_NOMOVE != 0 || !is_dialog(hwnd) {
        return None;
    }
    let size = if flags & SWP_NOSIZE == 0 {
        size
    } else {
        let r = window_rect(hwnd);
        (r[2], r[3])
    };
    let work = main_window().map_or([0, 0, size.0, size.1], |m| monitor_rects(m).1);
    Some(crate::virt::centred(crate::virt::frame_phys(), size, work))
}

fn current_style(hwnd: HWND) -> u32 {
    // SAFETY: style read for a window of this process.
    unsafe { GetWindowLongW(hwnd, GWL_STYLE) as u32 }
}

/// Windowed style: our decoration, the engine's other bits kept.
pub fn decorated_style(style: u32) -> u32 {
    (style & !WS_POPUP) | DECORATED
}

/// Fullscreen style: borderless popup over the monitor.
pub fn borderless_style(style: u32) -> u32 {
    (style & !DECORATED) | WS_POPUP
}

/// The monitor rect and its work area (`[x, y, w, h]`) of the monitor `hwnd` is on.
fn monitor_rects(hwnd: HWND) -> ([i32; 4], [i32; 4]) {
    // SAFETY: MONITORINFO is a plain out-struct; hwnd belongs to this process.
    let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    // SAFETY: info is a MONITORINFO with cbSize set.
    if mon.is_null() || unsafe { GetMonitorInfoW(mon, &raw mut info) } == 0 {
        let d = geom::desktop();
        return ([0, 0, d.0, d.1], [0, 0, d.0, d.1]);
    }
    let r = |r: RECT| [r.left, r.top, r.right - r.left, r.bottom - r.top];
    (r(info.rcMonitor), r(info.rcWork))
}

fn window_rect(hwnd: HWND) -> [i32; 4] {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: r is a valid RECT out-parameter.
    unsafe { GetWindowRect(hwnd, &raw mut r) };
    [r.left, r.top, r.right - r.left, r.bottom - r.top]
}

/// Our own host move/size: never sends `WINDOWPOSCHANGING` into the engine's window
/// procedure (it would run mid-blit), and takes the host out of the topmost band (the
/// engine created it topmost for its exclusive-fullscreen days).
fn set_pos(hwnd: HWND, r: [i32; 4]) {
    // SAFETY: documented call on a window of this process.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            r[0],
            r[1],
            r[2],
            r[3],
            SWP_NOACTIVATE | SWP_NOSENDCHANGING,
        );
    }
}

fn set_style(hwnd: HWND, style: u32) {
    if current_style(hwnd) == style {
        return;
    }
    // SAFETY: style write on a window of this process; the frame is re-computed below.
    unsafe { SetWindowLongW(hwnd, GWL_STYLE, style as i32) };
    // SAFETY: applies the new frame without moving or resizing the window.
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE
                | SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOZORDER
                | SWP_NOSENDCHANGING
                | SWP_FRAMECHANGED,
        );
    }
}
