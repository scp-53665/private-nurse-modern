//! `PN-Patch.exe` (`docs/04-l2-presenter.md` §1, §5): put it and `pn_modern.dll` in the
//! Private Nurse folder and run it — Install / Uninstall / Play, plus the patch settings.
//!
//! `PN-Patch.exe --install|--uninstall [--game DIR]` runs headless (exit code 0 = done);
//! the GUI uses it to re-run itself elevated when the game folder is read-only.
#![windows_subsystem = "windows"]

mod gui;
mod install;
mod win;

use std::path::PathBuf;

/// The game folder (`--game DIR`, else this EXE's folder) and the DLL to install (next to
/// this EXE).
pub struct Paths {
    pub game: PathBuf,
    pub payload: PathBuf,
}

fn paths(args: &[String]) -> Result<Paths, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let here = exe.parent().ok_or("no EXE folder")?.to_path_buf();
    // A missing value is a mistake, not a fallback: silently installing into PN-Patch's
    // own folder would "succeed" against the wrong directory (review s14 P-6).
    let game = match args.iter().position(|a| a == "--game") {
        Some(i) => args.get(i + 1).map_or_else(
            || Err("--game needs a folder".to_string()),
            |d| Ok(PathBuf::from(d)),
        )?,
        None => here.clone(),
    };
    Ok(Paths {
        game,
        payload: here.join(install::PAYLOAD),
    })
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let headless = |r: Result<String, String>| {
        if r.is_ok() {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        }
    };
    let Ok(p) = paths(&args) else {
        return std::process::ExitCode::FAILURE;
    };
    if args.iter().any(|a| a == "--install") {
        return headless(install::install(&p.game, &p.payload));
    }
    if args.iter().any(|a| a == "--uninstall") {
        return headless(install::uninstall(
            &p.game,
            &p.payload,
            pn_shared::appdata_path(),
        ));
    }
    match gui::run(p) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
