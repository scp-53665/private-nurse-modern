//! Win32 surface of the spike: type exports, string helpers, and raw PE-header reads.
//!
//! The PE reads are hand-rolled on purpose: the host's headers must be readable from
//! `DllMain`-spawned code before anything else is initialised, and `windows-sys` has
//! no `IMAGE_IMPORT_DESCRIPTOR` feature that is worth pulling for six offsets.

// Win32 ABI arithmetic and UTF-16 buffer lengths.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};

pub use windows_sys::Win32::Foundation::{FALSE, HINSTANCE, HMODULE, HWND, POINT, RECT, TRUE};
pub use windows_sys::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, DEVMODEA, DIBSECTION, GetDC, HBITMAP, HDC, ReleaseDC,
};
pub use windows_sys::Win32::System::Performance::{
    QueryPerformanceCounter, QueryPerformanceFrequency,
};
pub use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
/// `BOOL` lives in `windows_sys::core`; handles/types are split between Foundation
/// (HWND, HMODULE) and Gdi (HDC, HBITMAP) in this bindings version.
pub use windows_sys::core::BOOL;

/// Milliseconds since DLL load as `i64` (the sample-log integer type).
pub fn ms_i64() -> i64 {
    i64::try_from(crate::log::ms()).unwrap_or(i64::MAX)
}

/// Class name of a window, empty when it cannot be read.
///
/// # Safety
/// `h` must be a window handle in this process (a null handle yields an empty name).
pub unsafe fn class_of(h: HWND) -> String {
    if h.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 128];
    // SAFETY: buf is a writable UTF-16 buffer of the length passed.
    let n = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(
            h,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if n <= 0 {
        return String::new();
    }
    narrow(&buf)
}

/// QPC tick counter (the spike's cost measurement base).
pub fn qpc_now() -> i64 {
    let mut c = 0i64;
    // SAFETY: c is a valid i64 out-parameter.
    unsafe { QueryPerformanceCounter(&raw mut c) };
    c
}

/// Microseconds elapsed since `t0`, using the once-read QPC frequency.
pub fn qpc_us_since(t0: i64) -> u64 {
    static FREQ: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: f is a valid i64 out-parameter.
        unsafe { QueryPerformanceFrequency(&raw mut f) };
        u64::try_from(f).unwrap_or(1)
    });
    let d = qpc_now().saturating_sub(t0);
    (u64::try_from(d).unwrap_or(0) * 1_000_000) / freq
}

/// NUL-terminated UTF-16 for a `LPCWSTR` parameter.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// NUL-terminated bytes for a `LPCSTR`/`PCSTR` parameter (`GetProcAddress`).
pub fn narrow_bytes(s: &str) -> Vec<u8> {
    s.bytes().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer (as filled by the Win32 A/W calls) to a lossy `String`.
pub fn narrow(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Full path of a loaded module (`h` = 0 means the host EXE).
pub unsafe fn module_path(h: HINSTANCE) -> Option<String> {
    let mut buf = [0u16; 260];
    // SAFETY: buf is a writable 260-element UTF-16 buffer of the size we pass.
    let n = unsafe { GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 {
        return None;
    }
    let s = OsString::from_wide(&buf[..n as usize]);
    s.into_string().ok()
}

/// Base address of the host image (the EXE that loaded us).
pub unsafe fn host_base() -> *const u8 {
    // SAFETY: GetModuleHandleW(NULL) returns the EXE's base; never null in-process.
    unsafe { GetModuleHandleW(std::ptr::null()) }.cast::<u8>()
}

/// Directory the host EXE lives in: where the INI is looked for first (`config`).
pub fn game_dir() -> Option<std::path::PathBuf> {
    // SAFETY: null module handle means "this process's own image".
    let path = unsafe { module_path(std::ptr::null_mut()) }?;
    std::path::Path::new(&path)
        .parent()
        .map(std::path::Path::to_path_buf)
}

/// Aligned-safe 32/16-bit reads: the host's import table is 4-byte aligned in every
/// real image, but the spike must not fault if a malformed entry is walked.
///
/// # Safety
/// `p` must be readable for the read width.
pub unsafe fn r32(p: *const u8) -> u32 {
    // SAFETY: caller guarantees readability; unaligned read tolerates bad alignment.
    unsafe { p.cast::<u32>().read_unaligned() }
}

/// # Safety
/// See [`r32`].
pub unsafe fn r16(p: *const u8) -> u16 {
    // SAFETY: caller guarantees readability.
    unsafe { p.cast::<u16>().read_unaligned() }
}

/// `SizeOfImage` from the host's optional header.
///
/// # Safety
/// `base` must point at a mapped PE image.
pub unsafe fn image_size(base: *const u8) -> u32 {
    // SAFETY: the DOS header is at the image base of any loaded module.
    let e_lfanew = unsafe { r32(base.byte_add(0x3c)) } as usize;
    // Optional header starts after the 24-byte NT file header; SizeOfImage is +0x38.
    // SAFETY: in a loaded image the NT headers follow e_lfanew.
    unsafe { r32(base.add(e_lfanew + 0x18 + 0x38)) }
}

/// `(start, end)` of an image's address range.
///
/// # Safety
/// See [`image_size`].
pub unsafe fn image_range(base: *const u8) -> (usize, usize) {
    let start = base as usize;
    // SAFETY: forwarded from the caller's guarantee.
    (start, start + unsafe { image_size(base) } as usize)
}

/// True when `v` lies inside `[start, end)`.
pub fn in_range(v: usize, range: (usize, usize)) -> bool {
    v >= range.0 && v < range.1
}
