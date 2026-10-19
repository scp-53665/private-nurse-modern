//! Window census + subclass message log + the 1 Hz flush tick.
//!
//! This is the spike's answer to "which windows exist in each phase" (L2-M0 item 2,
//! closing the F15/U4 scope for L2) and the alt-tab mechanism question (O4: the
//! engine's own `ShowWindow`/`SetWindowPos` arguments are logged by the hooks; here
//! the style/position changes and the message sequence are recorded).

// Win32 ABI arithmetic and a window-procedure function pointer crossing the API
// boundary; both are inherent here. Justified once for the module.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::fn_to_numeric_cast,
    clippy::similar_names
)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, Sleep};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, EnumWindows, GWL_EXSTYLE, GWL_STYLE, GWL_WNDPROC,
    GetWindowLongW, GetWindowThreadProcessId, InternalGetWindowText, IsWindowVisible,
    SetWindowLongW, WINDOWPOS, WNDPROC,
};

use crate::frame;
use crate::hooks;
use crate::log;
use crate::win::{self, BOOL, HWND, RECT, TRUE};
use crate::window;

struct WinRec {
    /// Stored as an address: `HWND` is not `Send`, and this table lives in a `Mutex`.
    hwnd: usize,
    class: String,
    old_proc: isize,
    /// The window's DC as seen by `GetDC` (cached by GDI per window); recorded so the
    /// CSV can name it instead of repeating the HWND (review N2/N6 triage).
    dc: usize,
    state: String,
}

static WINS: Mutex<Vec<WinRec>> = Mutex::new(Vec::new());
static PAINTS: AtomicU32 = AtomicU32::new(0);
static ERASES: AtomicU32 = AtomicU32::new(0);
static MSG_LOGGED: AtomicU32 = AtomicU32::new(0);

/// Message name for the log, empty when the message is not in the interesting set.
fn msg_name(msg: u32) -> &'static str {
    match msg {
        0x0002 => "WM_DESTROY",
        0x0003 => "WM_MOVE",
        0x0005 => "WM_SIZE",
        0x0006 => "WM_ACTIVATE",
        0x0007 => "WM_SETFOCUS",
        0x0008 => "WM_KILLFOCUS",
        0x0010 => "WM_CLOSE",
        0x0018 => "WM_SHOWWINDOW",
        0x001c => "WM_ACTIVATEAPP",
        0x0021 => "WM_MOUSEACTIVATE",
        0x0024 => "WM_GETMINMAXINFO",
        0x0046 => "WM_WINDOWPOSCHANGING",
        0x0047 => "WM_WINDOWPOSCHANGED",
        0x007c => "WM_STYLECHANGING",
        0x007d => "WM_STYLECHANGED",
        0x007e => "WM_DISPLAYCHANGE",
        0x0082 => "WM_NCDESTROY",
        0x0083 => "WM_NCCALCSIZE",
        0x0085 => "WM_NCPAINT",
        0x0086 => "WM_NCACTIVATE",
        0x0100 => "WM_KEYDOWN",
        0x0102 => "WM_CHAR",
        0x0112 => "WM_SYSCOMMAND",
        0x0201 => "WM_LBUTTONDOWN",
        0x0202 => "WM_LBUTTONUP",
        0x0231 => "WM_ENTERSIZEMOVE",
        _ => "",
    }
}

fn state_of(hwnd: HWND) -> String {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: valid RECT out-parameter.
    unsafe { win::GetWindowRect(hwnd, &raw mut rect) };
    // SAFETY: window handles come from the census.
    let (style, ex) = unsafe {
        (
            GetWindowLongW(hwnd, GWL_STYLE),
            GetWindowLongW(hwnd, GWL_EXSTYLE),
        )
    };
    let visible = unsafe { IsWindowVisible(hwnd) };
    format!(
        "rect={},{},{},{} style={style:#010x} ex={ex:#010x} visible={visible}",
        rect.left, rect.top, rect.right, rect.bottom
    )
}

/// Records a window the first time it is seen and logs its spawn line.
fn note_window(hwnd: HWND) {
    if crate::stopping() {
        return; // detaching: do not take on new windows (review N9)
    }
    // Phase B policy: classify (and register) the game window and the text box. The
    // registry is filled lazily by every geometry/present hook too; this makes sure the
    // game window is known before the first frame needs its origin. Kill switch: with
    // `enabled=0` nothing enters V at all, not even for a spike run (review s14b m-10).
    if crate::config::enabled() {
        let _ = window::kind(hwnd);
    }
    // Deadlock rule (s13, pb15): this runs on the census thread, and `subclass_proc` locks
    // `WINS` on the engine's thread for every message. So `WINS` is never held across a
    // Win32 call, and the title is read with `InternalGetWindowText`, which — unlike
    // `GetWindowTextW` — does not *send* `WM_GETTEXT` to the owner thread. The old code
    // did both: census held `WINS` waiting for the engine to answer `WM_GETTEXT`, the
    // engine waited for `WINS` inside `subclass_proc`, and the game froze the moment a new
    // window (the text box, the ActiveMovie window) appeared.
    let known = |wins: &[WinRec]| wins.iter().any(|r| r.hwnd == hwnd as usize);
    if WINS.lock().is_ok_and(|w| known(&w)) {
        return;
    }
    let class = unsafe { win::class_of(hwnd) };
    let mut title_buf = [0u16; 128];
    // SAFETY: writable UTF-16 buffer of the size passed; sends no message.
    unsafe { InternalGetWindowText(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32) };
    let title = win::narrow(&title_buf);
    let state = state_of(hwnd);
    let dc = unsafe { win::GetDC(hwnd) } as usize;
    // SAFETY: pairs the GetDC above.
    unsafe { win::ReleaseDC(hwnd, dc as _) };
    // Only the census thread adds windows, so nobody can have added this one meanwhile;
    // the lock is taken for the install + push only (neither sends a message).
    let mut wins = WINS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if known(&wins) {
        return;
    }
    // SAFETY: subclass install; the previous proc is restored on detach.
    let proc_ptr: WNDPROC = Some(subclass_proc);
    let old =
        unsafe { SetWindowLongW(hwnd, GWL_WNDPROC, proc_ptr.map_or(0, |p| p as isize) as i32) };
    log::line(
        "win",
        format_args!(
            "new hwnd={:#x} class={class:?} title={title:?} dc={dc:#x} old_proc={old:#x} {state}",
            hwnd as usize
        ),
    );
    wins.push(WinRec {
        hwnd: hwnd as usize,
        class,
        old_proc: old as isize,
        dc,
        state,
    });
}

/// Puts the engine's window procedures back (best effort, on process detach).
pub fn restore() {
    // try_lock: on `ExitProcess` the census thread is already dead and may have died
    // holding `WINS`; waiting would hang the exit (review s12/s13 P2-7).
    let wins = match WINS.try_lock() {
        Ok(w) => w,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    for r in wins.iter() {
        if r.old_proc != 0 {
            // SAFETY: value came from SetWindowLongW for this same window.
            unsafe { SetWindowLongW(r.hwnd as HWND, GWL_WNDPROC, r.old_proc as i32) };
        }
    }
}

fn log_msg(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) {
    if msg == 0x000f {
        PAINTS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if msg == 0x0014 {
        ERASES.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let name = msg_name(msg);
    if name.is_empty() {
        return;
    }
    let n = MSG_LOGGED.fetch_add(1, Ordering::Relaxed);
    if n > 4000 {
        return;
    }
    let extra = if msg == 0x0046 || msg == 0x0047 {
        if l == 0 {
            String::new()
        } else {
            // SAFETY: WM_WINDOWPOS* passes a WINDOWPOS; read-only.
            let p = unsafe { &*(l as *const WINDOWPOS) };
            format!(
                " after={:#x} x={} y={} cx={} cy={} flags={:#x}",
                p.hwndInsertAfter as usize, p.x, p.y, p.cx, p.cy, p.flags
            )
        }
    } else {
        format!(" w={w:#x} l={l:#x}")
    };
    log::line(
        "msg",
        format_args!("hwnd={:#x} {name}{extra}", hwnd as usize),
    );
}

unsafe extern "system" fn subclass_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // The game window's messages are routed into V (`virt::route`): real geometry is
    // rewritten or swallowed, and the presenter answers its own commands. Everything else
    // is logged and forwarded to the engine. Kill switch: untouched.
    let routed = if crate::config::enabled() {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::virt::route(hwnd, msg, w, l)
        }))
        .unwrap_or(crate::virt::Route::Forward(w, l))
    } else {
        crate::virt::Route::Forward(w, l)
    };
    let (w, l) = match routed {
        crate::virt::Route::Swallow(result) => return result,
        crate::virt::Route::Forward(w, l) => (w, l),
    };
    if crate::log::spike() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| log_msg(hwnd, msg, w, l)));
    }
    let old = {
        let wins = WINS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        wins.iter()
            .find(|r| r.hwnd == hwnd as usize)
            .map_or(0, |r| r.old_proc)
    };
    if old == 0 {
        // SAFETY: default handling when the previous proc is unknown.
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    }
    // SAFETY: `old` is the engine's own window procedure for this window.
    let f: windows_sys::Win32::UI::WindowsAndMessaging::WNDPROC =
        unsafe { std::mem::transmute(old) };
    unsafe { CallWindowProcW(f, hwnd, msg, w, l) }
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut pid: u32 = 0;
    // SAFETY: valid u32 out-parameter.
    unsafe { GetWindowThreadProcessId(hwnd, &raw mut pid) };
    if pid as usize == lparam as usize {
        note_window(hwnd);
    }
    TRUE
}

/// Logs per-window state changes (rect/style/visibility) since the previous tick.
fn log_changes() {
    let mut wins = WINS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for r in wins.iter_mut() {
        let now = state_of(r.hwnd as HWND);
        if now != r.state {
            log::line(
                "win",
                format_args!("changed hwnd={:#x} class={:?} {now}", r.hwnd, r.class),
            );
            r.state = now;
        }
    }
}

/// Spike-only: sample a present target's window DC, at most once a second.
///
/// Called from the present hook, i.e. on the window's **owning thread** (see
/// [`sample_windows`] for why the census thread does not sample the box).
///
/// A screen-DC capture of the same window would show frame *and* box together, but
/// in-process screen reads are unreliable in this environment: the spike's screen-DC shot
/// of the game window came back as "caption + one strip + black" while the window DC had
/// the frame (pb7 `shot` lines vs the `win` samples). `tools/shot.py` takes the composited
/// picture from outside the process instead.
pub fn sample_present_target(hwnd: HWND, src: crate::win::HDC) {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = log::ms();
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < 1000 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    // The engine's own source for this blit, next to the window's own pixels: a source
    // that renders while the window stays black means the sink lost the pixels (the
    // window DC read can also go black when a window is not composited, so both are
    // needed to tell the two apart).
    // SAFETY: src is the DC the engine passed to its own blit.
    if let Some(s) = unsafe { frame::observe_dc(src, "boxsrc") } {
        log::line(
            "boxsrc",
            format_args!(
                "{}x{} bpp={} hash={:016x} dumped={}",
                s.w,
                s.h,
                s.bpp,
                s.hash,
                u8::from(s.file.is_some())
            ),
        );
    }
    // SAFETY: hwnd is a present target of this process; same-thread GetDC.
    if let Some(c) = unsafe { frame::observe_window(hwnd, "box") } {
        log::line(
            "box",
            format_args!(
                "hwnd={:#x} {}x{} hash={:016x} dumped={}",
                hwnd as usize,
                c.w,
                c.h,
                c.hash,
                u8::from(c.file.is_some())
            ),
        );
    }
}

/// The 1 Hz tick: census, per-window state, screen-path frame sample, counter flush.
pub fn run(stop: &AtomicBool) {
    if !crate::config::enabled() && !log::spike() {
        // Kill switch (review s12/s13 P2-5): no window subclassing at all.
        return;
    }
    let pid = unsafe { GetCurrentProcessId() };
    let mut ticks: u32 = 0;
    log::line("census", format_args!("start pid={pid} tick=1s"));
    // The 1-hour cap bounds a spike's log; a normal run keeps the census for the whole
    // session, because it is what subclasses windows created later (the text box is
    // re-created per dialogue and needs the window policy).
    while !stop.load(Ordering::Relaxed) && (ticks < 3600 || !log::spike()) {
        // SAFETY: enumeration with our own callback; no locks held across it.
        unsafe { EnumWindows(Some(enum_cb), pid as LPARAM) };
        log_changes();
        if log::spike() {
            sample_windows();
        }
        hooks::flush_counted();
        crate::cost::flush(ticks);
        if log::spike() {
            log::line(
                "tick",
                format_args!(
                    "t={}s windows={} paint={} erase={} msgs={} frames_dumped={}",
                    log::ms() / 1000,
                    WINS.lock().map_or(0, |w| w.len()),
                    PAINTS.load(Ordering::Relaxed),
                    ERASES.load(Ordering::Relaxed),
                    MSG_LOGGED.load(Ordering::Relaxed),
                    frame::dumped()
                ),
            );
        }
        ticks += 1;
        // SAFETY: plain Win32 sleep in our own thread.
        unsafe { Sleep(1000) };
    }
    log::line("census", format_args!("stop after {ticks} ticks"));
}

/// Screen-path time series: the pixels of the game window, 1 Hz.
///
/// Only the *game window* is sampled here; the box is sampled on the engine's own thread
/// ([`sample_present_target`]). Session 12 blamed `GetDC` for the census stopping the
/// second the box appeared (pb4); session 13 found the real mechanism — `note_window`
/// held `WINS` across `GetWindowTextW` (a cross-thread `WM_GETTEXT`) while the engine
/// waited for `WINS` in `subclass_proc` (pb15 vs pb16). The same-thread rule is kept as
/// the cheaper default, not because `GetDC` was shown to block.
fn sample_windows() {
    let candidates: Vec<(HWND, usize)> = {
        let wins = WINS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        wins.iter()
            .map(|r| (r.hwnd as HWND, r.dc))
            // SAFETY: handles from the census.
            .filter(|(h, _)| unsafe { IsWindowVisible(*h) != 0 })
            .filter(|(h, _)| window::kind(*h) == Some(window::Kind::Main))
            .collect()
    };
    for (hwnd, dc) in candidates.iter().take(2) {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: valid RECT out-parameter.
        unsafe { win::GetWindowRect(*hwnd, &raw mut rect) };
        if rect.right - rect.left < 64 || rect.bottom - rect.top < 64 {
            continue;
        }
        // SAFETY: hwnd comes from the census, inside this process.
        if let Some(c) = unsafe { frame::observe_window(*hwnd, "win") } {
            // Same column layout as the blt rows (hooks::csv_header); a census row
            // carries the window's DC as `src_hdc` and its HWND as `dest_win`, no rop.
            log::csv(format_args!(
                "{},win,{:#x},0,{},{},{},0,0,0,0,0,0,0,0,0,{dc:#x},{:#x},,{:016x},{},,{}",
                log::ms(),
                *dc,
                c.bpp,
                c.w,
                c.h,
                *hwnd as usize,
                c.hash,
                u8::from(c.file.is_some()),
                c.us
            ));
        }
    }
}
