//! The two Win32 calls PN-Patch needs: "is the game running?" and "run me elevated".

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// Lower-cased, canonical, verbatim-prefix-free form of a path, for comparing image paths.
fn norm(p: &Path) -> String {
    let c = std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    let s = c.to_string_lossy().to_lowercase();
    s.strip_prefix(r"\\?\")
        .map_or_else(|| s.clone(), str::to_string)
}

/// Is process `pid` running the image `exe`? Unknown (access denied) counts as yes.
fn runs_image(pid: u32, exe: &Path) -> bool {
    // SAFETY: documented call; the handle is checked and closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return true;
    }
    let mut buf = [0u16; 1024];
    let mut n = u32::try_from(buf.len()).unwrap_or(0);
    // SAFETY: `buf` holds `n` UTF-16 units; `h` is a live process handle.
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &raw mut n) } != 0;
    // SAFETY: handle from OpenProcess, closed once.
    unsafe { CloseHandle(h) };
    !ok || norm(Path::new(&String::from_utf16_lossy(&buf[..n as usize]))) == norm(exe)
}

/// True if a process is running the image `exe` (this exact file, not just its name: a
/// copy of the game in another folder does not block this folder; review s14 S14-6).
pub fn game_running(exe: &Path) -> bool {
    let name = exe.file_name().map(|n| n.to_string_lossy().into_owned());
    let Some(name) = name else { return false };
    // SAFETY: documented call; the handle is checked and closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut e = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0),
        ..Default::default()
    };
    let mut found = false;
    // SAFETY: `snap` is a valid snapshot and `e` is a properly sized entry.
    let mut more = unsafe { Process32FirstW(snap, &raw mut e) } != 0;
    while more && !found {
        let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
        found = String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(&name)
            && runs_image(e.th32ProcessID, exe);
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap, &raw mut e) } != 0;
    }
    // SAFETY: handle from CreateToolhelp32Snapshot, closed once.
    unsafe { CloseHandle(snap) };
    found
}

/// Runs this EXE again with `args` through the UAC prompt and waits for it.
/// Returns its exit code; `Err` if the user declined or the launch failed.
pub fn run_elevated(args: &str) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let (verb, file, params) = (
        wide("runas".as_ref()),
        wide(exe.as_os_str()),
        wide(args.as_ref()),
    );
    let mut info = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: params.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // SAFETY: every pointer in `info` outlives the call.
    if unsafe { ShellExecuteExW(&raw mut info) } == 0 || info.hProcess.is_null() {
        return Err("elevation was declined or failed".into());
    }
    let mut code = 1u32;
    // SAFETY: `hProcess` is a live process handle we own (NOCLOSEPROCESS).
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        GetExitCodeProcess(info.hProcess, &raw mut code);
        CloseHandle(info.hProcess);
    }
    Ok(code)
}

/// Quotes a path for a command line (paths cannot contain `"`). Backslashes before the
/// closing quote are doubled, or `"D:\"` would read back as `D:"` (review s14 S14-2).
pub fn quoted(p: &Path) -> String {
    let s = p.display().to_string();
    let tail = s.len() - s.trim_end_matches('\\').len();
    format!("\"{s}{}\"", "\\".repeat(tail))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // AGENTS: unwrap only in tests
mod tests {
    use super::*;

    #[test]
    fn game_running_matches_the_image_path_not_just_the_name() {
        let me = std::env::current_exe().unwrap();
        assert!(game_running(&me));
        let name = me.file_name().unwrap();
        let elsewhere = std::env::temp_dir().join("pn_patch_nowhere").join(name);
        assert!(!game_running(&elsewhere), "same image name, other folder");
    }

    #[test]
    fn quoted_survives_a_trailing_backslash() {
        assert_eq!(quoted(Path::new(r"C:\Games\PN")), r#""C:\Games\PN""#);
        assert_eq!(quoted(Path::new(r"D:\")), r#""D:\\""#);
    }
}
