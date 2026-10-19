//! The engine's virtual 640x480 monitor, *V* (`docs/04-l2-presenter.md` §3.0, user decision
//! session 14).
//!
//! The engine runs in its original fullscreen setup: its game window is a borderless
//! 640x480 window at V (0,0), so V screen = game-window client = frame logical space. Every
//! geometry answer the engine gets is in V. Every geometry request it makes is recorded
//! here and never applied to a real window. Global side effects are refused (`vhooks`).
//!
//! **The real boundaries**, the only places V meets physical pixels, all go through
//! [`to_phys`]/[`to_v`] (frame origin on screen + v * scale):
//! * the popup menu position (`TrackPopupMenu`) and `SetCursorPos`;
//! * the cursor reads (`GetCursorPos`, `GetMessagePos`), in V unless an engine dialog or
//!   menu is the active window ([`cursor_in_v`]);
//! * mouse messages to the game window, and to the text box while the engine holds capture
//!   on it ([`route`]: the box's real window is unscaled and unmoved, session 15);
//! * the engine's dialogs: real windows, placed by `window::place_dialog`;
//! * `Client`/`ScreenToClient` and `MapWindowPoints` involving one of those real windows.
//!
//! Not virtualised, with the evidence that says so (review s14b): `GetUpdateRect`
//! (imported, one engine call site at `0x40dbdc`; now counted as an observer and
//! measured at **0 calls** in run vd7, so the leak is code-read only), `ValidateRect`
//! (MFC, clears a region in the space it came from), `GetClipBox`,
//! `PtVisible`/`RectVisible`, `AdjustWindowRect[Ex]`, `RedrawWindow` (**0 call sites**,
//! verified by the IAT walk), `ScrollWindowEx`, and GDI primitives drawn straight into a
//! managed window's DC (`WM_PAINT` never reached one in any run: the frame arrives as an
//! intercepted blit). A new boundary is added here, not as a one-off hook.

// Win32 lParam packing and i32<->f64 point math.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::sync::Mutex;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetActiveWindow, GetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, SWP_NOMOVE, SWP_NOSIZE, SendMessageW, WS_DISABLED,
    WS_MAXIMIZE, WS_MINIMIZE, WS_VISIBLE,
};

use crate::geom;
use crate::win::HWND;
use crate::window::{self, Kind};

/// V screen `[x, y, w, h]`: the whole virtual monitor.
pub const SCREEN: [i32; 4] = [0, 0, geom::FRAME.0, geom::FRAME.1];

/// Engine commands the presenter answers itself (menu 131 of the EXE, `RT_MENU`).
pub const CMD_WINDOW_MODE: u16 = 2001;
pub const CMD_DEFAULT_POSITION: u16 = 32772;

/// What the engine believes about one of its windows.
struct VWin {
    hwnd: usize,
    /// V screen `[x, y, w, h]`.
    rect: [i32; 4],
    style: i32,
    ex: i32,
}

static VWINS: Mutex<Vec<VWin>> = Mutex::new(Vec::new());

fn with<R>(f: impl FnOnce(&mut Vec<VWin>) -> R) -> R {
    f(&mut VWINS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// First sight of a managed window. The game window is the whole V screen; the text box is
/// where the engine created it (its real rect, which nothing has moved yet: every engine
/// move goes through [`set_rect`] from now on).
pub fn adopt(hwnd: HWND, kind: Kind) {
    let rect = match kind {
        Kind::Main => SCREEN,
        Kind::Box => {
            let mut r = crate::win::RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            // SAFETY: valid RECT out-parameter; our own (real) call, not the hook.
            unsafe { crate::win::GetWindowRect(hwnd, &raw mut r) };
            [r.left, r.top, r.right - r.left, r.bottom - r.top]
        }
    };
    // SAFETY: style reads for a window of this process.
    let (style, ex) = unsafe {
        (
            GetWindowLongW(hwnd, GWL_STYLE),
            GetWindowLongW(hwnd, GWL_EXSTYLE),
        )
    };
    with(|v| {
        v.retain(|w| w.hwnd != hwnd as usize);
        if v.len() >= 32 {
            v.remove(0); // the box is re-created per dialogue: a cache, not a log
        }
        v.push(VWin {
            hwnd: hwnd as usize,
            rect,
            style,
            ex,
        });
    });
    crate::log::line(
        "virt",
        format_args!("adopt hwnd={:#x} {kind:?} v={rect:?}", hwnd as usize),
    );
}

/// The V rect of a managed window, or of the desktop window (the V screen).
pub fn rect(hwnd: HWND) -> Option<[i32; 4]> {
    if is_desktop(hwnd) {
        return Some(SCREEN);
    }
    window::kind(hwnd)?;
    with(|v| v.iter().find(|w| w.hwnd == hwnd as usize).map(|w| w.rect))
}

/// Records an engine move/size. The game window stays the V screen whatever it asks.
pub fn set_rect(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32, flags: u32) {
    if window::kind(hwnd) != Some(Kind::Box) {
        return;
    }
    with(|v| {
        if let Some(w) = v.iter_mut().find(|w| w.hwnd == hwnd as usize) {
            if flags & SWP_NOMOVE == 0 {
                (w.rect[0], w.rect[1]) = (x, y);
            }
            if flags & SWP_NOSIZE == 0 {
                (w.rect[2], w.rect[3]) = (cx, cy);
            }
        }
    });
}

/// Bits of the real style that are *state*, not the engine's choice.
const STATE_BITS: i32 = (WS_VISIBLE | WS_MINIMIZE | WS_MAXIMIZE | WS_DISABLED) as i32;

/// `GetWindowLongA(GWL_STYLE|GWL_EXSTYLE)` as the engine set it (state bits live).
pub fn style(hwnd: HWND, index: i32) -> Option<i32> {
    if index != GWL_STYLE && index != GWL_EXSTYLE {
        return None;
    }
    let stored = with(|v| {
        v.iter()
            .find(|w| w.hwnd == hwnd as usize)
            .map(|w| if index == GWL_STYLE { w.style } else { w.ex })
    })?;
    if index == GWL_EXSTYLE {
        return Some(stored);
    }
    // SAFETY: style read for a window of this process.
    let real = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
    Some((stored & !STATE_BITS) | (real & STATE_BITS))
}

/// Records `SetWindowLongA(GWL_STYLE|GWL_EXSTYLE)`; returns the previous V value.
pub fn set_style(hwnd: HWND, index: i32, value: i32) -> Option<i32> {
    let previous = style(hwnd, index)?;
    with(|v| {
        if let Some(w) = v.iter_mut().find(|w| w.hwnd == hwnd as usize) {
            if index == GWL_STYLE {
                w.style = value;
            } else {
                w.ex = value;
            }
        }
    });
    Some(previous)
}

fn is_desktop(hwnd: HWND) -> bool {
    // SAFETY: no arguments; returns a process-independent handle.
    !hwnd.is_null()
        && hwnd == unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }
}

// ---------------------------------------------------------------------------
// The transform: V <-> physical screen
// ---------------------------------------------------------------------------

/// Physical screen position of V (0,0): the game window's client origin plus the plan's
/// letterbox offset. Without a game window yet, V and physical coincide.
fn frame_origin() -> (i32, i32) {
    window::main_window().map_or((0, 0), |main| {
        let (x, y) = geom::client_origin(main);
        let o = geom::plan().origin;
        (x + o.0, y + o.1)
    })
}

pub fn to_phys(p: (i32, i32)) -> (i32, i32) {
    geom::to_physical(frame_origin(), geom::scale(), p)
}

pub fn to_v(p: (i32, i32)) -> (i32, i32) {
    geom::to_logical(frame_origin(), geom::scale(), p)
}

/// The frame's physical screen rect `[x, y, w, h]` (where dialogs are centred).
pub fn frame_phys() -> [i32; 4] {
    geom::rect_to_physical(frame_origin(), geom::scale(), SCREEN)
}

/// The cursor is in V while the engine's own world is in front; when one of its dialogs
/// is active, MFC code in that dialog works in real coordinates (the game loop is
/// blocked in the modal loop meanwhile).
pub fn cursor_in_v() -> bool {
    // SAFETY: no arguments; the calling (engine) thread's active window.
    let active = unsafe { GetActiveWindow() };
    active.is_null() || window::kind(active).is_some()
}

// ---------------------------------------------------------------------------
// Messages to the game window
// ---------------------------------------------------------------------------

/// What the subclass does with one message.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Answered here; the engine never sees it.
    Swallow(LRESULT),
    /// Passed to the engine, possibly rewritten into V.
    Forward(WPARAM, LPARAM),
}

const WM_MOVE: u32 = 0x0003;
const WM_SIZE: u32 = 0x0005;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_WINDOWPOSCHANGING: u32 = 0x0046;
const WM_WINDOWPOSCHANGED: u32 = 0x0047;
const WM_DISPLAYCHANGE: u32 = 0x007E;
const WM_CONTEXTMENU: u32 = 0x007B;
const WM_COMMAND: u32 = 0x0111;
const SIZE_MINIMIZED: usize = 1;

/// The game window's (and the text box's) messages, routed into V. Only messages that
/// carry real geometry, or that the presenter answers itself, are touched.
pub fn route(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Route {
    let Some(kind) = window::kind(hwnd) else {
        return Route::Forward(w, l);
    };
    if msg == WM_ERASEBKGND {
        return Route::Swallow(1); // "painted": the scaled frame and its bars stay put
    }
    if kind != Kind::Main {
        // The text box gets mouse messages only while the engine holds capture on it (it
        // is click-through otherwise): the page-break wait and the choices hit-test them
        // against its 632x104 client (session 15: `WM_LBUTTONUP l=955,593` never matched).
        let mapped = if is_client_mouse(msg) {
            rect(hwnd).map(|r| {
                let real = geom::client_origin(hwnd);
                let at = other_client_to_v(unpack(l), real, frame_origin(), geom::scale());
                pack((at.0 - r[0], at.1 - r[1]))
            })
        } else if is_screen_mouse(msg) && l != -1 {
            Some(pack(to_v(unpack(l))))
        } else {
            None
        };
        return Route::Forward(w, mapped.unwrap_or(l));
    }
    match msg {
        // The game window never moves in V; a real move (user drag, our own placement)
        // must not reach the engine's handler, which re-places the text box (pb23-pb25).
        // WINDOWPOSCHANGING/CHANGED carry the *physical* outer rect (vd5: x=317 y=40
        // cx=1286 cy=1000 in the same batch as a correctly virtualised WM_SIZE), so they
        // are the message form of an un-virtualised `GetWindowRect` (review s14b M-1).
        WM_MOVE | WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED | WM_DISPLAYCHANGE => {
            Route::Swallow(0)
        }
        WM_SIZE if w != SIZE_MINIMIZED => Route::Forward(w, pack(geom::FRAME)),
        WM_COMMAND => command(w, l),
        m if is_client_mouse(m) => match to_box(hwnd, m, w, l) {
            Some(r) => Route::Swallow(r),
            None => Route::Forward(w, mouse_lparam(m, l, &geom::plan()).unwrap_or(l)),
        },
        m if is_screen_mouse(m) => {
            Route::Forward(w, mouse_lparam(m, l, &geom::plan()).unwrap_or(l))
        }
        _ => Route::Forward(w, l),
    }
}

fn command(w: WPARAM, l: LPARAM) -> Route {
    match (w & 0xFFFF) as u16 {
        CMD_WINDOW_MODE => {
            window::toggle_mode();
            Route::Swallow(0)
        }
        CMD_DEFAULT_POSITION => {
            window::recentre();
            Route::Swallow(0)
        }
        _ => Route::Forward(w, l),
    }
}

/// `WM_MOUSEMOVE` .. `WM_MBUTTONDBLCLK`, `WM_XBUTTON*`: client coordinates.
fn is_client_mouse(msg: u32) -> bool {
    (0x0200..=0x0209).contains(&msg) || (0x020B..=0x020D).contains(&msg)
}

/// `WM_MOUSEWHEEL`, `WM_MOUSEHWHEEL`, `WM_CONTEXTMENU`: screen coordinates.
fn is_screen_mouse(msg: u32) -> bool {
    msg == 0x020A || msg == 0x020E || msg == WM_CONTEXTMENU
}

/// A mouse message's `lParam`, mapped into V: client messages through the plan,
/// screen messages through the frame transform. `None`: not a mouse message, or the
/// documented "no coordinates" case (`WM_MOUSEWHEEL`/`WM_CONTEXTMENU` from the
/// keyboard, `lParam == -1`), which the caller forwards unchanged.
fn mouse_lparam(msg: u32, l: LPARAM, plan: &geom::Plan) -> Option<LPARAM> {
    if is_client_mouse(msg) {
        Some(pack(client_to_v(unpack(l), plan)))
    } else if is_screen_mouse(msg) && l != -1 {
        Some(pack(to_v(unpack(l))))
    } else {
        None
    }
}

/// A physical client point of the game window -> V (= the frame's logical space).
pub fn client_to_v(p: (i32, i32), plan: &geom::Plan) -> (i32, i32) {
    geom::to_logical(plan.origin, plan.scale, p)
}

/// A mouse message over the visible text box goes to the box, as it did when the box was a
/// real window on top of the game (it is click-through here): its own handlers do the
/// choice hover and click (message map `0x44de18`: `WM_MOUSEMOVE` `0x40e5f0`, `WM_LBUTTONUP`
/// `0x40e4f0`; session 15, the choices did not react). Not while the game window holds
/// capture. Sent in the box's real client coordinates, which its own route maps into V.
fn to_box(main: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    // Right buttons stay with the game window: the box's own handlers (`0x40e300`,
    // `0x40e390`) only re-post them to it (`PostMessageA` at `0x40e37a`/`0x40e40a`) with a
    // point already in V, which this route would map a second time (session 15: the menu
    // opened up-left of the cursor). Delivering them here directly is the same result.
    // SAFETY: plain query on the calling (engine) thread.
    if (0x0204..=0x0206).contains(&msg) || unsafe { GetCapture() } == main {
        return None;
    }
    let bx = window::visible_box()?;
    let bounds = rect(bx)?;
    let at_v = client_to_v(unpack(lparam), &geom::plan());
    let [bx0, by0, bw, bh] = bounds;
    if at_v.0 < bx0 || at_v.1 < by0 || at_v.0 >= bx0 + bw || at_v.1 >= by0 + bh {
        return None;
    }
    let (host, real_box) = (geom::client_origin(main), geom::client_origin(bx));
    let p = unpack(lparam);
    let at = pack((p.0 + host.0 - real_box.0, p.1 + host.1 - real_box.1));
    // SAFETY: the box belongs to this (the engine's) thread, so this is a direct call into
    // its window procedure; no presenter lock is held here (trap T1).
    Some(unsafe { SendMessageW(bx, msg, wparam, at) })
}

/// A client point of a real (unscaled, unmoved) engine window, as USER32 delivers it ->
/// the V screen point under the same physical pixel.
fn other_client_to_v(
    p: (i32, i32),
    real_origin: (i32, i32),
    frame_origin: (i32, i32),
    scale: f64,
) -> (i32, i32) {
    geom::to_logical(
        frame_origin,
        scale,
        (p.0 + real_origin.0, p.1 + real_origin.1),
    )
}

/// Signed 16-bit halves, as `GET_X_LPARAM`/`GET_Y_LPARAM`.
pub fn unpack(l: LPARAM) -> (i32, i32) {
    (
        i32::from(l as u16 as i16),
        i32::from((l >> 16) as u16 as i16),
    )
}

pub fn pack(p: (i32, i32)) -> LPARAM {
    ((u32::from(p.1 as u16) << 16) | u32::from(p.0 as u16)) as i32 as LPARAM
}

/// Top-left of a `size` window centred over `over`, kept inside `work` (`[x, y, w, h]`).
pub fn centred(over: [i32; 4], size: (i32, i32), work: [i32; 4]) -> (i32, i32) {
    let x = over[0] + (over[2] - size.0) / 2;
    let y = over[1] + (over[3] - size.1) / 2;
    let x = x.min(work[0] + work[2] - size.0).max(work[0]);
    let y = y.min(work[1] + work[3] - size.1).max(work[1]);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lparam_round_trips_signed_points() {
        for p in [(0, 0), (639, 479), (-5, -7), (1386, 1039), (-32000, -32000)] {
            assert_eq!(unpack(pack(p)), p);
        }
    }

    #[test]
    fn a_physical_client_click_lands_on_the_same_v_point() {
        // DPI-aware windowed fit (pb35) and fullscreen with bars (pb36)
        let windowed = geom::Plan {
            scale: 2.166_667,
            client: (1387, 1040),
            origin: (0, 0),
        };
        assert_eq!(client_to_v((693, 520), &windowed), (320, 240));
        let full = geom::Plan {
            scale: 2.25,
            client: (1920, 1080),
            origin: (240, 0),
        };
        assert_eq!(client_to_v((240 + 720, 540), &full), (320, 240));
        assert_eq!(client_to_v((0, 0), &full).0, -107); // the left bar is off-frame
    }

    #[test]
    fn mouse_message_classes() {
        for m in [0x0200, 0x0201, 0x0204, 0x0205, 0x0209, 0x020B, 0x020D] {
            assert!(is_client_mouse(m), "{m:#x}");
        }
        for m in [0x020A, 0x020E, 0x007B] {
            assert!(is_screen_mouse(m) && !is_client_mouse(m), "{m:#x}");
        }
        assert!(!is_client_mouse(0x0084)); // WM_NCHITTEST stays real: caption drags
    }

    /// The click rewrite itself (review s14b test-pin audit: dropping it passed every
    /// test). Physical client point -> V, including the letterbox and the keyboard case.
    #[test]
    fn mouse_lparam_maps_clicks_into_v() {
        let full = geom::Plan {
            scale: 2.25,
            client: (1920, 1080),
            origin: (240, 0),
        };
        let click = |p| {
            mouse_lparam(0x0201, pack(p), &full)
                .map_or_else(|| panic!("not a mouse message {p:?}"), unpack)
        };
        assert_eq!(click((240 + 720, 540)), (320, 240));
        assert_eq!(click((248, 846)), (4, 376)); // 4*2+240, 376*2.25
        // a click in the left letterbox bar arrives out of frame: the engine's own
        // hit test drops it (`IG` 0x85 does PtInRect on V), which is the agreed answer
        // (docs/04 §3.0)
        assert_eq!(click((100, 540)), (-62, 240));
        // lParam == -1: the keyboard case, forwarded unchanged
        assert_eq!(mouse_lparam(0x007B, -1, &full), None);
        assert_eq!(mouse_lparam(0x020A, -1, &full), None);
    }

    /// Session 15 run s15c8: the box's real window sits at (4,376) unscaled (`create hwnd=0x7711b0
    /// … at=4,376,632,104`) and the captured click arrives as `WM_LBUTTONUP l=0x25103bb` =
    /// (955,593) on that same hwnd, far outside its 632x104 client.
    ///
    /// `origin` is the *client's* screen position, not the `plan` origin (that is 0,0).
    /// x = 263 + (1393-1387)/2, from the logged host rect `[263, 0, 1393, 1080]` and client
    /// 1387x1040 (even side borders). y is INFERRED: even chrome would give (1080-1040)/2 = 20,
    /// not the 37 the run's click implies — the host window's vertical chrome is not even.
    /// Scale 1040/480 = 2.166667 (`plan … scale=2.166667`).
    #[test]
    fn a_captured_box_click_maps_into_the_box_client() {
        let (origin, scale) = ((266, 37), 1040.0 / 480.0);
        let v = other_client_to_v((955, 593), (4, 376), origin, scale);
        assert_eq!((v.0 - 4, v.1 - 376), (316, 54));
    }

    #[test]
    fn dialogs_centre_over_the_frame_and_stay_on_screen() {
        let frame = [0, 37, 1387, 1040];
        let work = [0, 0, 1920, 1080];
        assert_eq!(centred(frame, (561, 591), work), (413, 261));
        // a dialog taller than the work area keeps its title bar on screen
        assert_eq!(centred(frame, (561, 1200), work).1, 0);
        // a frame near the right edge pulls the dialog back inside
        assert_eq!(centred([1700, 0, 600, 400], (561, 300), work).0, 1920 - 561);
        // wider than the work area: it cannot fit, so it keeps the left edge (the
        // ceiling of this clamp, review s14b n-4)
        assert_eq!(centred(frame, (2400, 300), work), (0, 407));
    }
}
