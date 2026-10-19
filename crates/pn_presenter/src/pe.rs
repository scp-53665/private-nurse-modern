//! Host import-table walk and the order-agnostic IAT hook install (review R1/NF-2).
//!
//! The snap test is `slot value == GetProcAddress(module, function)`, not "the value
//! changed": a `DllMain`-time read may already be the snapped pointer, and a
//! change-based wait then silently no-ops into fail-open. Equally, a not-yet-snapped
//! slot holds an image RVA that is *outside* the image range, so "outside the image"
//! is not a snap test either.

// Win32 ABI arithmetic over image offsets, plus IAT slots, which the PE format
// guarantees to be 4-byte aligned.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_ptr_alignment
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::Sleep;

use crate::log;
use crate::win;
use crate::win::HMODULE;

pub struct HookSpec {
    pub module: &'static str,
    pub func: &'static str,
    pub hook: usize,
    pub orig: &'static AtomicUsize,
}

impl HookSpec {
    pub const fn new(
        module: &'static str,
        func: &'static str,
        hook: usize,
        orig: &'static AtomicUsize,
    ) -> Self {
        Self {
            module,
            func,
            hook,
            orig,
        }
    }
}

/// Case-insensitive compare of an in-image NUL-terminated name with `want`, bounded
/// by the image range so a malformed table cannot fault the game.
///
/// # Safety
/// `p` must point into the host image described by `range`.
unsafe fn name_eq(p: *const u8, want: &str, range: (usize, usize)) -> bool {
    for (i, w) in want.bytes().enumerate() {
        if !win::in_range(p as usize + i, range) {
            return false;
        }
        // SAFETY: the index was just range-checked against a mapped image.
        let c = unsafe { p.add(i).read() };
        if c == 0 || !c.eq_ignore_ascii_case(&w) {
            return false;
        }
    }
    // SAFETY: same bound as above, for the terminator.
    win::in_range(p as usize + want.len(), range) && unsafe { p.add(want.len()).read() } == 0
}

/// Address of the host's IAT slot for `module!func`, walked from the PE headers.
///
/// Bounds: the descriptor walk is limited to the import data directory's own extent
/// (`Size` field) and every read is range-checked first — a malformed table must not
/// fault inside someone else's process (an AV is not a catchable panic).
///
/// # Safety
/// `host` must be the base of the process's main image.
pub unsafe fn find_slot(host: *const u8, module: &str, func: &str) -> Option<*mut u32> {
    let range = unsafe { win::image_range(host) };
    // SAFETY: DOS header at the image base.
    let e_lfanew = unsafe { win::r32(host.byte_add(0x3c)) } as usize;
    let opt = unsafe { host.add(e_lfanew + 0x18) };
    // SAFETY: PE32 magic check before reading a PE32-layout data directory.
    if unsafe { win::r16(opt) } != 0x10b {
        return None;
    }
    // Import directory entry: data directory #1, 8 bytes each (RVA + size).
    // SAFETY: inside the optional header of a PE32 image.
    let (import_rva, import_size) = unsafe {
        (
            win::r32(opt.add(0x60 + 8)) as usize,
            win::r32(opt.add(0x60 + 12)) as usize,
        )
    };
    let table_lo = host as usize + import_rva;
    if import_rva == 0 || import_size == 0 || !win::in_range(table_lo, range) {
        return None;
    }
    let table_hi = table_lo.saturating_add(import_size).min(range.1);
    let mut d = table_lo;
    loop {
        if d + 20 > table_hi {
            return None; // ran off the import directory without a terminator
        }
        // SAFETY: d+20 is inside the directory extent, hence inside the image.
        let fields = unsafe {
            (
                win::r32((d as *const u8).add(12)) as usize,
                win::r32((d as *const u8).add(16)) as usize,
                win::r32(d as *const u8) as usize,
            )
        };
        let (name_rva, first_thunk, oft) = fields;
        if name_rva == 0 || first_thunk == 0 {
            return None; // terminator
        }
        if win::in_range(host as usize + name_rva, range)
            && win::in_range(host as usize + first_thunk, range)
            && unsafe { name_eq(host.add(name_rva), module, range) }
        {
            if oft == 0 {
                // No name table (bound import): only ordinals could be matched, which
                // this walk does not support — say so instead of misreading the
                // CurrentThunkValues table.
                log::line(
                    "iat",
                    format_args!("{module} has no OriginalFirstThunk; descriptor skipped"),
                );
            } else if let Some(slot) = unsafe { thunks(host, oft, first_thunk, func, range) } {
                return Some(slot);
            }
        }
        d += 20;
    }
}

/// Walks one descriptor's lookup table (`OriginalFirstThunk`) for `func`; returns its
/// IAT slot. Every entry read is range-checked; the walk is bounded by the image and a
/// hard entry cap.
///
/// # Safety
/// `oft`/`first_thunk` are RVAs inside the image described by `range`.
unsafe fn thunks(
    host: *const u8,
    oft: usize,
    first_thunk: usize,
    func: &str,
    range: (usize, usize),
) -> Option<*mut u32> {
    let lo = host as usize + oft;
    if !win::in_range(lo, range) {
        return None;
    }
    for i in 0..4096usize {
        let entry = lo + i * 4;
        if !win::in_range(entry, range) || !win::in_range(entry + 4, range) {
            return None;
        }
        // SAFETY: `entry` was range-checked above.
        let t = unsafe { win::r32(entry as *const u8) } as usize;
        if t == 0 {
            return None;
        }
        if t & 0x8000_0000 == 0
            && win::in_range(host as usize + t + 2, range)
            // SAFETY: bounded above; +2 skips the IMAGE_IMPORT_BY_NAME hint word.
            && unsafe { name_eq(host.add(t + 2), func, range) }
        {
            return if win::in_range(host as usize + first_thunk + i * 4, range) {
                Some(
                    unsafe { host.add(first_thunk + i * 4) }
                        .cast_mut()
                        .cast::<u32>(),
                )
            } else {
                None
            };
        }
    }
    None
}

/// `GetProcAddress` on an already-loaded module, as an address (0 when unresolved).
fn resolve(module: &str, func: &str) -> usize {
    let name = win::wide(module);
    // SAFETY: wide() returns a NUL-terminated UTF-16 string.
    let h: HMODULE = unsafe { GetModuleHandleW(name.as_ptr()) };
    if h.is_null() {
        return 0;
    }
    let f = win::narrow_bytes(func);
    // SAFETY: f is a NUL-terminated ANSI name.
    unsafe { GetProcAddress(h, f.as_ptr()) }.map_or(0, |p| p as usize)
}

/// Classifies a slot for the log (R1 evidence: what the value looked like when the
/// loader got there).
fn state(
    v: usize,
    expect: usize,
    hook: usize,
    host_size: usize,
    ours: (usize, usize),
) -> &'static str {
    if v == 0 {
        "zero"
    } else if v == hook || win::in_range(v, ours) {
        "ours"
    } else if v == expect {
        "snapped"
    } else if v < host_size {
        "unsnapped-rva"
    } else {
        "unknown-pointer"
    }
}

/// Finds every slot, waits (bounded) for each to hold its snapped pointer, then
/// patches it. Fail-open: anything still unresolved is logged and left alone.
///
/// # Safety
/// `host` must be the main image base and `ours` this DLL's range.
pub fn install(
    specs: &[HookSpec],
    host: *const u8,
    ours: (usize, usize),
    timeout: Duration,
) -> u32 {
    let host_size = unsafe { win::image_size(host) } as usize;
    let deadline = Instant::now() + timeout;
    let mut pending: Vec<(*mut u32, usize)> = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        let expect = resolve(spec.module, spec.func);
        // SAFETY: forwarded from the caller.
        let slot = unsafe { find_slot(host, spec.module, spec.func) };
        let Some(slot) = slot else {
            log::line(
                "iat",
                format_args!(
                    "NO-SLOT {}!{} — not imported by the host, skipped",
                    spec.module, spec.func
                ),
            );
            continue;
        };
        // SAFETY: slot is inside the host image.
        let v = unsafe { slot.read() } as usize;
        log::line(
            "iat",
            format_args!(
                "{}!{} slot={:#x} value={:#x} expect={:#x} state={}",
                spec.module,
                spec.func,
                slot as usize,
                v,
                expect,
                state(v, expect, spec.hook, host_size, ours)
            ),
        );
        pending.push((slot, i));
    }
    let installed = wait_and_patch(specs, pending, deadline, ours);
    log::line(
        "install",
        format_args!("{installed}/{} slots hooked", specs.len()),
    );
    installed
}

/// The snapped-pointer wait loop. Split out to keep each step reviewable.
fn wait_and_patch(
    specs: &[HookSpec],
    mut pending: Vec<(*mut u32, usize)>,
    deadline: Instant,
    ours: (usize, usize),
) -> u32 {
    let mut installed = 0;
    let start = Instant::now();
    while !pending.is_empty() && Instant::now() < deadline {
        let mut still = Vec::with_capacity(pending.len());
        for (slot, i) in pending {
            let spec = &specs[i];
            // SAFETY: slot came from the host's import table; expect may be 0 while a
            // statically-imported module is still being brought in by the loader.
            let expect = resolve(spec.module, spec.func);
            let v = unsafe { slot.read() } as usize;
            if v == spec.hook || win::in_range(v, ours) {
                // Our own hook already in place (double load / partial tick).
                installed += 1;
                continue;
            }
            if expect == 0 || v != expect {
                still.push((slot, i));
                continue;
            }
            spec.orig.store(v, Ordering::Release);
            // SAFETY: single 32-bit store into the host's IAT slot.
            unsafe { slot.write(u32::try_from(spec.hook).unwrap_or(u32::MAX)) };
            let back = unsafe { slot.read() } as usize;
            if back == spec.hook {
                installed += 1;
                log::line(
                    "install",
                    format_args!(
                        "{}!{} hooked slot={:#x} orig={v:#x} after_ms={}",
                        spec.module,
                        spec.func,
                        slot as usize,
                        start.elapsed().as_millis()
                    ),
                );
            } else {
                log::line(
                    "install",
                    format_args!(
                        "{}!{} write did not stick (read back {back:#x}) — left as is",
                        spec.module, spec.func
                    ),
                );
            }
        }
        pending = still;
        if !pending.is_empty() {
            // SAFETY: plain Win32 sleep; no loader lock is held by this thread.
            unsafe { Sleep(20) };
        }
    }
    for (slot, i) in pending {
        // SAFETY: read-only check on a host IAT slot.
        let v = unsafe { slot.read() } as usize;
        let spec = &specs[i];
        log::line(
            "install",
            format_args!(
                "FAIL-OPEN {}!{} slot={:#x} value={v:#x} after {}ms",
                spec.module,
                spec.func,
                slot as usize,
                start.elapsed().as_millis()
            ),
        );
    }
    installed
}
