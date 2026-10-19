//! Single-window composition (session-13 user requirement: the dialogue text box is part
//! of the game window — two top-level windows drift apart when the game window is moved
//! or minimised).
//!
//! The engine keeps drawing into its two windows. The presenter keeps one logical layer
//! per window — `frame` (the game window's 640x480) and `boxl` (the text box's client) —
//! composes them into `comp` (the frame, then the box on top while the box window is
//! alive and visible, at its V rect), and hands only `comp` to the sink, for the game
//! window. The engine's box window stays alive for the engine's own logic but is invisible
//! and click-through (`window::hide_box`).
//!
//! All of this runs on the engine's thread (the present and `ShowWindow` hooks); the
//! lock is never held across a call that sends a message.

// GDI handle/integer packing.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::sync::Mutex;

use windows_sys::Win32::Graphics::Gdi::{COLORONCOLOR, SRCCOPY, SetStretchBltMode, StretchBlt};
use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, IsWindowVisible};

use crate::geom;
use crate::sink::{self, Present};
use crate::virt;
use crate::win::{HDC, HWND};
use crate::window;

/// A presenter-owned 640x480 logical surface (a DIB section in its own memory DC,
/// `sink::new_dib`). Lives for the process; three of them exist.
struct Layer(usize);

/// Only reached when `State::new` fails part-way (a later layer's DIB): the layers made so
/// far are freed instead of leaked on every retry.
impl Drop for Layer {
    fn drop(&mut self) {
        sink::free_dib(self.dc(), std::ptr::null_mut());
    }
}

impl Layer {
    fn new() -> Option<Self> {
        // A DIB section, so the sink can scale its pixels itself (`resample`). Owned by
        // this layer for the life of the process.
        let (dc, _) = sink::new_dib(geom::FRAME.0, geom::FRAME.1)?;
        // SAFETY: dc is live; the engine's own stretches are reproduced pixel-exact.
        unsafe { SetStretchBltMode(dc, COLORONCOLOR) };
        Some(Self(dc as usize))
    }

    fn dc(&self) -> HDC {
        self.0 as HDC
    }

    /// Copies `src_rect` of `src` to `dst` of this layer, 1:1 in logical units unless the
    /// engine itself stretched (then its own stretch is reproduced).
    fn draw(&self, dst: [i32; 4], src: HDC, src_rect: [i32; 4]) -> bool {
        // SAFETY: both DCs are live; GDI clips to the layer's bitmap.
        unsafe {
            StretchBlt(
                self.dc(),
                dst[0],
                dst[1],
                dst[2],
                dst[3],
                src,
                src_rect[0],
                src_rect[1],
                src_rect[2],
                src_rect[3],
                SRCCOPY,
            ) != 0
        }
    }
}

struct State {
    frame: Layer,
    boxl: Layer,
    comp: Layer,
    /// The engine's text box window, if one has presented.
    boxwin: usize,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// One blit the engine aimed at a managed window.
pub struct Blit {
    pub hwnd: HWND,
    pub src: HDC,
    pub src_rect: [i32; 4],
    /// Destination in the target window's logical client space.
    pub dst: [i32; 4],
}

/// Takes one engine blit into the composition and presents the changed region of the
/// game window. `false` means nothing was presented (the caller passes the blit through).
pub fn present(blit: &Blit) -> bool {
    let Some(main) = window::main_window() else {
        return false;
    };
    let mut guard = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_none() {
        *guard = State::new();
    }
    let Some(state) = guard.as_mut() else {
        return false;
    };
    let region = if window::kind(blit.hwnd) == Some(window::Kind::Box) {
        state.boxwin = blit.hwnd as usize;
        if !state.boxl.draw(blit.dst, blit.src, blit.src_rect) {
            return false;
        }
        let r = state.box_rect();
        clip(
            [
                blit.dst[0] + r[0],
                blit.dst[1] + r[1],
                blit.dst[2],
                blit.dst[3],
            ],
            r,
        )
    } else {
        if !state.frame.draw(blit.dst, blit.src, blit.src_rect) {
            return false;
        }
        blit.dst
    };
    state.present(main, region)
}

/// The engine showed, hid or moved its text box: re-present the box's area.
pub fn box_changed(hwnd: HWND) {
    let Some(main) = window::main_window() else {
        return;
    };
    let mut guard = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(state) = guard.as_mut()
        && state.boxwin == hwnd as usize
    {
        let r = state.box_rect();
        state.present(main, r);
    }
}

/// The host's layout changed (mode switch, first placement): re-present the whole frame.
pub fn refresh() {
    let Some(main) = window::main_window() else {
        return;
    };
    let mut guard = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(state) = guard.as_mut() {
        state.present(main, virt::SCREEN);
    }
}

impl State {
    fn new() -> Option<Self> {
        Some(Self {
            frame: Layer::new()?,
            boxl: Layer::new()?,
            comp: Layer::new()?,
            boxwin: 0,
        })
    }

    /// Where the box sits in the frame: its V rect (V screen is frame space, the game
    /// window being the whole V screen), read live so an engine move of the box is honoured.
    fn box_rect(&self) -> [i32; 4] {
        virt::rect(self.boxwin as HWND).unwrap_or([0, 0, 0, 0])
    }

    fn box_live(&mut self) -> bool {
        if self.boxwin == 0 {
            return false;
        }
        let h = self.boxwin as HWND;
        // SAFETY: plain queries; a stale handle just reads as "not a window".
        let live = unsafe { IsWindow(h) != 0 && IsWindowVisible(h) != 0 };
        // SAFETY: as above.
        if !live && unsafe { IsWindow(h) } == 0 {
            self.boxwin = 0; // destroyed: the frame shows through from now on
        }
        live
    }

    /// Recomposes `region` (frame, then the visible box on top) and presents it.
    fn present(&mut self, main: HWND, region: [i32; 4]) -> bool {
        if region[2] <= 0 || region[3] <= 0 {
            return true;
        }
        self.comp.draw(region, self.frame.dc(), region);
        if self.box_live() {
            let b = self.box_rect();
            let over = clip(region, b);
            if over[2] > 0 && over[3] > 0 {
                let from = [over[0] - b[0], over[1] - b[1], over[2], over[3]];
                self.comp.draw(over, self.boxl.dc(), from);
            }
        }
        sink::sink().present(&Present {
            hwnd: main,
            origin: window::client_logical_origin(main),
            src: self.comp.dc(),
            src_rect: region,
            logical: region,
        })
    }
}

/// Intersection of two `[x, y, w, h]` rects (empty: zero width or height).
fn clip(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    [x0, y0, (x1 - x0).max(0), (y1 - y0).max(0)]
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn clip_intersects_and_empties() {
        assert_eq!(
            clip([0, 0, 640, 480], [4, 376, 632, 104]),
            [4, 376, 632, 104]
        );
        assert_eq!(clip([0, 0, 640, 100], [4, 376, 632, 104]), [4, 376, 632, 0]);
        assert_eq!(
            clip([10, 380, 20, 22], [4, 376, 632, 104]),
            [10, 380, 20, 22]
        );
    }
}
