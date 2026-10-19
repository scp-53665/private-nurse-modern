//! Lazy forwarding of the three proxy exports to the stock shim, which the installer
//! (or the README's manual steps) renames to `fakecdda_real.dll`.
//!
//! Nothing here runs in `DllMain`: the real DLL is loaded on the first forwarded call,
//! and a missing real DLL logs and fails the call (review Y3 — the WINMM fallback is
//! not behaviour-preserving: real CD-DA instead of the MP3 shim).

// Win32 ABI arithmetic for handle/pointer round-trips.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::log;
use crate::win::{self, HWND};

/// Stock shim file name after the rename.
const REAL_NAME: &str = "fakecdda_real.dll";
/// `MCIERR_UNSUPPORTED_FUNCTION` — mmsyscom.h `MCIERR_BASE` 256 + mciapi.h +18.
const MCI_UNSUPPORTED: u32 = 274;
/// `MMSYSERR_NOTSUPPORTED` — mmsyscom.h `MMSYSERR_BASE` 0 + 8.
const MMSYS_NOTSUPPORTED: u32 = 8;

const NAMES: [&str; 3] = [
    "mciSendCommandA",
    "mciSendStringA",
    "mixerSetControlDetails",
];
static CALLS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];

static REAL: OnceLock<usize> = OnceLock::new();
static P_MCI_SEND_COMMAND: AtomicUsize = AtomicUsize::new(0);
static P_MCI_SEND_STRING: AtomicUsize = AtomicUsize::new(0);
static P_MIXER_SET_CONTROL_DETAILS: AtomicUsize = AtomicUsize::new(0);

type MciSendCommandFn = unsafe extern "system" fn(u32, u32, usize, usize) -> u32;
type MciSendStringFn = unsafe extern "system" fn(*const u8, *mut u8, u32, HWND) -> u32;
type MixerSetControlDetailsFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> u32;

/// Directory of this DLL (where the renamed stock shim must sit).
fn our_dir() -> Option<PathBuf> {
    let inst = crate::our_instance() as win::HINSTANCE;
    let path = unsafe { win::module_path(inst) }?;
    PathBuf::from(path)
        .parent()
        .map(std::path::Path::to_path_buf)
}

/// Address of the stock shim's module, loaded once.
fn real() -> usize {
    *REAL.get_or_init(|| {
        let Some(dir) = our_dir() else {
            log::line(
                "proxy",
                format_args!("cannot resolve own directory; forwarding disabled"),
            );
            return 0;
        };
        let path = dir.join(REAL_NAME);
        let wide = win::wide(&path.to_string_lossy());
        // SAFETY: wide is a NUL-terminated UTF-16 path.
        let h = unsafe { LoadLibraryW(wide.as_ptr()) };
        if h.is_null() {
            log::line(
                "proxy",
                format_args!(
                    "FAIL-OPEN {REAL_NAME} missing next to the game EXE ({})",
                    path.display()
                ),
            );
            0
        } else {
            log::line(
                "proxy",
                format_args!("loaded {} handle={:#x}", path.display(), h as usize),
            );
            h as usize
        }
    })
}

/// One-time `GetProcAddress` per forwarded export.
fn resolve(cache: &AtomicUsize, name: &[u8]) -> usize {
    let v = cache.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    let h = real();
    if h == 0 {
        return 0;
    }
    // SAFETY: h is the module handle returned by LoadLibraryW; name is NUL-terminated.
    let p = unsafe { GetProcAddress(h as win::HMODULE, name.as_ptr()) };
    let v = p.map_or(0, |f| f as usize);
    if v != 0 {
        cache.store(v, Ordering::Relaxed);
    } else {
        log::line("proxy", format_args!("export missing in {REAL_NAME}"));
    }
    v
}

fn note(idx: usize, ret: u32) {
    let n = CALLS[idx].fetch_add(1, Ordering::Relaxed);
    if n < 50 {
        log::line("proxy", format_args!("{} #{n} ret={ret}", NAMES[idx]));
    }
}

/// `MCIERROR mciSendCommandA(MCIDEVICEID, UINT, DWORD_PTR, DWORD_PTR)`.
///
/// # Safety
/// Called by the host through its import table with the stock shim's calling
/// convention; arguments are forwarded unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn mciSendCommandA(id: u32, msg: u32, p1: usize, p2: usize) -> u32 {
    let p = resolve(&P_MCI_SEND_COMMAND, b"mciSendCommandA\0");
    if p == 0 {
        note(0, MCI_UNSUPPORTED);
        return MCI_UNSUPPORTED;
    }
    // SAFETY: resolved from the stock shim's export of the same name.
    let f: MciSendCommandFn = unsafe { std::mem::transmute(p) };
    let r = unsafe { f(id, msg, p1, p2) };
    note(0, r);
    r
}

/// `MCIERROR mciSendStringA(LPCSTR, LPSTR, UINT, HWND)`.
///
/// # Safety
/// See [`mciSendCommandA`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn mciSendStringA(
    cmd: *const u8,
    ret: *mut u8,
    cch: u32,
    cb: HWND,
) -> u32 {
    let p = resolve(&P_MCI_SEND_STRING, b"mciSendStringA\0");
    if p == 0 {
        note(1, MCI_UNSUPPORTED);
        return MCI_UNSUPPORTED;
    }
    // SAFETY: resolved from the stock shim's export of the same name.
    let f: MciSendStringFn = unsafe { std::mem::transmute(p) };
    let r = unsafe { f(cmd, ret, cch, cb) };
    note(1, r);
    r
}

/// `MMRESULT mixerSetControlDetails(HMIXEROBJ, LPMIXERCONTROLDETAILS, DWORD)`.
///
/// # Safety
/// See [`mciSendCommandA`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn mixerSetControlDetails(
    obj: *mut c_void,
    details: *mut c_void,
    flags: u32,
) -> u32 {
    let p = resolve(&P_MIXER_SET_CONTROL_DETAILS, b"mixerSetControlDetails\0");
    if p == 0 {
        note(2, MMSYS_NOTSUPPORTED);
        return MMSYS_NOTSUPPORTED;
    }
    // SAFETY: resolved from the stock shim's export of the same name.
    let f: MixerSetControlDetailsFn = unsafe { std::mem::transmute(p) };
    let r = unsafe { f(obj, details, flags) };
    note(2, r);
    r
}
