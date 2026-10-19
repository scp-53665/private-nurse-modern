//! Install / uninstall on a game folder (`docs/04-l2-presenter.md` §1).
//!
//! Install: the stock `fakecdda.dll` becomes `fakecdda_real.dll` (our DLL forwards to it)
//! and our `pn_modern.dll` is copied in as `fakecdda.dll`. Uninstall reverses it and
//! removes the INIs. Both are idempotent and refuse while the game runs. No manifest and no
//! change to `NURSEUS.EXE` (A2: the DLL sets DPI awareness itself).

use std::path::{Path, PathBuf};

pub const GAME_EXE: &str = "NURSEUS.EXE";
pub const SHIM: &str = "fakecdda.dll";
pub const REAL: &str = "fakecdda_real.dll";
pub const PAYLOAD: &str = "pn_modern.dll";
/// A string only our DLL carries (its log banner); the stock `FakeCDDA` shim has none.
const MARKER: &[u8] = b"pn_modern";

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// `current`: the installed DLL is byte-identical to the payload next to PN-Patch.
    Installed {
        current: bool,
    },
    Broken(String),
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

pub fn is_ours(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|b| contains(&b, MARKER))
}

pub fn status(dir: &Path, payload: &Path) -> Status {
    let (shim, real) = (dir.join(SHIM), dir.join(REAL));
    match (shim.exists(), real.exists(), is_ours(&shim)) {
        (true, true, true) => Status::Installed {
            current: std::fs::read(&shim).ok() == std::fs::read(payload).ok(),
        },
        (true, false, true) => Status::Broken(format!(
            "{SHIM} is ours but {REAL} is missing: delete {SHIM} by hand, or put the stock \
             {SHIM} back and run Uninstall again"
        )),
        (true, false, false) => Status::NotInstalled,
        (true, true, false) => Status::Broken(format!(
            "{REAL} exists but {SHIM} is not the patch DLL; restore one of them by hand"
        )),
        (false, true, _) => Status::Broken(format!(
            "{SHIM} is missing ({REAL} present): rename {REAL} back to {SHIM} by hand"
        )),
        (false, false, _) => Status::Broken(format!("no {SHIM}: is this the game folder?")),
    }
}

/// The game folder must hold the EXE that imports `fakecdda.dll`, and the game must not run.
fn check_game(dir: &Path) -> Result<(), String> {
    let exe = std::fs::read(dir.join(GAME_EXE))
        .map_err(|e| format!("{GAME_EXE} not found in {}: {e}", dir.display()))?;
    // ponytail: byte search for the import name, not a PE import walk; enough to reject a
    // wrong folder (the US EXE names it once, E01). Walk the import table if that misfires.
    if !contains(&exe, SHIM.as_bytes()) {
        return Err(format!(
            "{GAME_EXE} does not import {SHIM}: unsupported game version"
        ));
    }
    if crate::win::game_running(&dir.join(GAME_EXE)) {
        return Err("Private Nurse is running: close it first".into());
    }
    Ok(())
}

pub fn install(dir: &Path, payload: &Path) -> Result<String, String> {
    check_game(dir)?;
    if !is_ours(payload) {
        return Err(format!(
            "{} is missing or is not the patch DLL",
            payload.display()
        ));
    }
    let (shim, real) = (dir.join(SHIM), dir.join(REAL));
    let renamed = match status(dir, payload) {
        Status::NotInstalled => {
            std::fs::rename(&shim, &real).map_err(|e| format!("rename {SHIM} -> {REAL}: {e}"))?;
            true
        }
        Status::Installed { .. } => false,
        Status::Broken(why) => return Err(why),
    };
    let tmp = dir.join("fakecdda.dll.new");
    let copied = std::fs::copy(payload, &tmp).and_then(|_| std::fs::rename(&tmp, &shim));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&tmp);
        let mut msg = format!("copy {PAYLOAD} -> {SHIM}: {e}");
        if renamed {
            // Put the stock shim back: the game must keep working after a failed install.
            if let Err(e2) = std::fs::rename(&real, &shim) {
                msg = format!("{msg}; could not restore {SHIM} ({e2}): rename {REAL} back by hand");
            }
        }
        return Err(msg);
    }
    Ok(if renamed { "Installed." } else { "Updated." }.into())
}

/// `appdata_ini`: the per-user INI to remove too (`pn_shared::appdata_path()`; tests pass
/// `None` so they never touch the real profile).
pub fn uninstall(
    dir: &Path,
    payload: &Path,
    appdata_ini: Option<PathBuf>,
) -> Result<String, String> {
    if crate::win::game_running(&dir.join(GAME_EXE)) {
        return Err("Private Nurse is running: close it first".into());
    }
    match status(dir, payload) {
        Status::Installed { .. } => {}
        Status::NotInstalled => return Ok("Not installed: nothing to do.".into()),
        Status::Broken(why) => return Err(why),
    }
    let (shim, real) = (dir.join(SHIM), dir.join(REAL));
    // One replacing rename, not remove + rename: if the rename fails (a scanner holding
    // the stock shim open) our DLL must stay in place, not leave the folder without a
    // `fakecdda.dll` (review s14 S14-1).
    std::fs::rename(&real, &shim).map_err(|e| format!("rename {REAL} -> {SHIM}: {e}"))?;
    for ini in std::iter::once(pn_shared::portable_path(dir)).chain(appdata_ini) {
        let _ = std::fs::remove_file(ini);
    }
    Ok("Uninstalled: the original files are back.".into())
}

/// Can we write here without elevation (Program Files: no)?
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(".pn_patch_write_test");
    let ok = std::fs::File::create_new(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Where Save writes: next to the game when that folder is writable, else `%APPDATA%`
/// (the DLL reads the portable INI first, then the `%APPDATA%` one).
pub fn save_target(dir: &Path) -> Option<PathBuf> {
    if writable(dir) {
        Some(pn_shared::portable_path(dir))
    } else {
        pn_shared::appdata_path()
    }
}

/// The INI the game will actually read when it is not `target` (the portable INI wins over
/// `%APPDATA%`, so a stale one next to the game silently overrides a Save to `%APPDATA%`;
/// review s14 S14-4).
pub fn shadowed_by(dir: &Path, target: &Path) -> Option<PathBuf> {
    pn_shared::existing_path(Some(dir)).filter(|active| active != target)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // AGENTS: unwrap only in tests
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    /// A fake game folder: an "EXE" naming the shim, a stock shim, and a payload.
    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("pn_patch_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join(GAME_EXE), b"MZ...FAKECDDA.dll...").unwrap();
        std::fs::write(dir.join(SHIM), b"stock shim").unwrap();
        let payload = dir.join("dist").join(PAYLOAD);
        std::fs::write(&payload, b"...pn_modern v1...").unwrap();
        (dir, payload)
    }

    #[test]
    fn install_update_uninstall_round_trip() {
        let (dir, payload) = fixture("round");
        assert_eq!(status(&dir, &payload), Status::NotInstalled);
        assert_eq!(install(&dir, &payload).unwrap(), "Installed.");
        assert_eq!(status(&dir, &payload), Status::Installed { current: true });
        assert_eq!(std::fs::read(dir.join(REAL)).unwrap(), b"stock shim");
        std::fs::write(&payload, b"...pn_modern v2...").unwrap();
        assert_eq!(status(&dir, &payload), Status::Installed { current: false });
        assert_eq!(install(&dir, &payload).unwrap(), "Updated.");
        assert_eq!(
            std::fs::read(dir.join(SHIM)).unwrap(),
            b"...pn_modern v2..."
        );
        std::fs::write(pn_shared::portable_path(&dir), "enabled=0\n").unwrap();
        uninstall(&dir, &payload, None).unwrap();
        assert_eq!(std::fs::read(dir.join(SHIM)).unwrap(), b"stock shim");
        assert!(!dir.join(REAL).exists());
        assert!(!pn_shared::portable_path(&dir).exists());
        assert_eq!(status(&dir, &payload), Status::NotInstalled);
        assert!(
            uninstall(&dir, &payload, None)
                .unwrap()
                .starts_with("Not installed")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_portable_ini_shadows_an_appdata_save() {
        let (dir, _) = fixture("shadow");
        let portable = pn_shared::portable_path(&dir);
        let appdata = dir.join("appdata.ini");
        std::fs::write(&portable, "enabled=0\n").unwrap();
        assert_eq!(shadowed_by(&dir, &portable), None);
        assert_eq!(shadowed_by(&dir, &appdata), Some(portable));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_uninstall_keeps_our_shim_in_place() {
        let (dir, payload) = fixture("lock");
        install(&dir, &payload).unwrap();
        // Hold the stock shim open without FILE_SHARE_DELETE (a std File shares delete,
        // so use the raw share mode 0x3 = read | write).
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(dir.join(REAL))
            .unwrap();
        assert!(uninstall(&dir, &payload, None).is_err());
        assert_eq!(status(&dir, &payload), Status::Installed { current: true });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refusals_leave_the_folder_untouched() {
        let (dir, payload) = fixture("refuse");
        std::fs::write(dir.join(GAME_EXE), b"MZ other game").unwrap();
        assert!(
            install(&dir, &payload)
                .unwrap_err()
                .contains("does not import")
        );
        std::fs::write(dir.join(GAME_EXE), b"MZ fakecdda.dll").unwrap();
        std::fs::write(&payload, b"not ours").unwrap();
        assert!(
            install(&dir, &payload)
                .unwrap_err()
                .contains("is not the patch DLL")
        );
        assert_eq!(std::fs::read(dir.join(SHIM)).unwrap(), b"stock shim");
        std::fs::remove_file(dir.join(SHIM)).unwrap();
        assert!(matches!(status(&dir, &payload), Status::Broken(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
