//! Private Nurse L2 presenter (`docs/04-l2-presenter.md`).
//!
//! Loaded as `fakecdda.dll` (the stock shim renamed `fakecdda_real.dll`), the DLL does
//! three things:
//!
//! 1. forwards the three proxy exports lazily (`proxy`);
//! 2. presents the engine's 640x480 frame scaled into its own window (`present`, `sink`,
//!    `geom`, `window`) — mode switch suppressed, dialogue text box included;
//! 3. installs hooks and, only when a run asks for them (`PN_LOG_DIR`), the Phase A
//!    observers (`pe`, `hooks`, `census`, `frame`).
//!
//! `enabled=0` in the INI turns 2 off completely: the engine behaves as if the DLL were
//! not there (`config`).

mod census;
mod compose;
mod config;
mod cost;
mod count;
mod frame;
mod geom;
mod hooks;
mod log;
mod pe;
mod present;
mod proxy;
mod resample;
mod sink;
mod vhooks;
mod virt;
mod win;
mod window;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use windows_sys::Win32::System::Threading::CreateThread;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_SYSTEM_AWARE, SetProcessDpiAwarenessContext,
};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;
/// Bounded wait for the host's IAT snap (review R1/NF-2).
const SNAP_TIMEOUT: Duration = Duration::from_secs(5);

static OUR_INSTANCE: AtomicUsize = AtomicUsize::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
/// `log::init` succeeded in `DllMain`; without a log the spike thread does nothing.
static LOG_OK: AtomicBool = AtomicBool::new(false);

/// This DLL's `HINSTANCE`, stored by `DllMain` (equal to its load address).
pub fn our_instance() -> usize {
    OUR_INSTANCE.load(Ordering::Relaxed)
}

/// True once `DLL_PROCESS_DETACH` has run; the census stops taking on windows then.
pub fn stopping() -> bool {
    STOP.load(Ordering::Relaxed)
}

/// # Safety
/// Win32 entry point: called by the loader with the documented arguments.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: win::HINSTANCE,
    reason: u32,
    _reserved: *mut std::ffi::c_void,
) -> win::BOOL {
    // AGENTS hard rule: no unwinding across an FFI boundary — not even the loader's. A
    // panic during attach must still load us: refusing to load the shim would leave the
    // game unable to start, and the presenter is optional (review s14 P-1).
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        attach_or_detach(module, reason)
    }))
    .unwrap_or(if reason == DLL_PROCESS_ATTACH {
        win::TRUE
    } else {
        win::FALSE
    })
}

/// The loader's attach/detach work: memory reads plus `CreateThread` (review Y2).
fn attach_or_detach(module: win::HINSTANCE, reason: u32) -> win::BOOL {
    match reason {
        DLL_PROCESS_ATTACH => {
            OUR_INSTANCE.store(module as usize, Ordering::Relaxed);
            // SAFETY: documented call; our DLL has no thread-local notifications to
            // receive, and skipping them keeps the loader lock free.
            unsafe { DisableThreadLibraryCalls(module) };
            // The INI is read here, not on the spike thread: `dpi_aware` must act before
            // the engine's `WinMain` creates a window (we are an import of the EXE).
            // File I/O only — no loader-lock hazard.
            LOG_OK.store(log::init().is_ok(), Ordering::Relaxed);
            config::load();
            dpi_early();
            let mut tid = 0u32;
            // SAFETY: the entry point takes an unused parameter and never unwinds.
            let thread = unsafe {
                CreateThread(
                    std::ptr::null(),
                    0,
                    Some(spike_thread),
                    std::ptr::null_mut(),
                    0,
                    &raw mut tid,
                )
            };
            if thread.is_null() {
                // A dead thread means a silently useless spike; say so (review N9).
                log::line("start", format_args!("CreateThread FAILED: no spike"));
            } else {
                // We never join it; closing the handle just releases the reference.
                // SAFETY: handle came from CreateThread and is not used again.
                unsafe { CloseHandle(thread) };
            }
        }
        DLL_PROCESS_DETACH => {
            STOP.store(true, Ordering::Relaxed);
            // Best effort, so a staged-game restart does not inherit our windows.
            census::restore();
        }
        _ => {}
    }
    win::TRUE
}

unsafe extern "system" fn spike_thread(_param: *mut std::ffi::c_void) -> u32 {
    // A panic must never cross the thread-start frame.
    if std::panic::catch_unwind(spike_main).is_err() {
        log::line(
            "panic",
            format_args!("spike thread aborted; the game continues"),
        );
    }
    0
}

/// A2 (DLL route): process-wide system DPI awareness, set before any window
/// exists. Fails (logged) when a manifest or compatibility flag already chose one.
fn dpi_early() {
    let cfg = config::current();
    // No log = `spike_main` installs no hooks: the engine must then run exactly as stock,
    // not DPI-aware with an unscaled 640x480 window (review s14 S14-5).
    if !(LOG_OK.load(Ordering::Relaxed) && cfg.enabled && cfg.dpi_aware) {
        return;
    }
    // SAFETY: documented call with a documented constant; user32 is initialised before
    // us (it is one of our imports) and the call only sets a process flag.
    let ok = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE) };
    // Last-error is only meaningful on failure (it is stale otherwise).
    // SAFETY: plain thread-local read.
    let err = if ok == 0 {
        unsafe { windows_sys::Win32::Foundation::GetLastError() }
    } else {
        0
    };
    log::line("dpi", format_args!("system-aware ok={ok} err={err}"));
}

fn spike_main() {
    if !LOG_OK.load(Ordering::Relaxed) {
        // Without a log there is no evidence; the run recipe treats this as failure.
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        log::line("panic", format_args!("{info}"));
    }));
    // Phase B: the session plan (scale/mode/filter) is decided once, before any hook can
    // ask a geometry question, and it never changes afterwards (INI read in `DllMain`).
    geom::init();
    // SAFETY: reads the host EXE's own module base.
    let host = unsafe { win::host_base() };
    let ours = our_instance() as *const u8;
    // SAFETY: both bases are loaded images of this process.
    let host_range = unsafe { win::image_range(host) };
    // SAFETY: as above.
    let our_range = unsafe { win::image_range(ours) };
    log::line(
        "start",
        format_args!(
            "pn_modern log dir={} host={:#x}+{:#x} ours={:#x}+{:#x} pid={}",
            log::dir().display(),
            host_range.0,
            host_range.1 - host_range.0,
            our_range.0,
            our_range.1 - our_range.0,
            std::process::id()
        ),
    );
    log::line(
        "start",
        format_args!("host_path={:?}", unsafe {
            win::module_path(std::ptr::null_mut())
        }),
    );
    hooks::csv_header();
    // SAFETY: host/our ranges come from the loaded images themselves.
    let installed = pe::install(hooks::hooks(), host, our_range, SNAP_TIMEOUT);
    log::line(
        "start",
        format_args!("hooks installed {installed}/{}", hooks::hooks().len()),
    );
    census::run(&STOP);
}
