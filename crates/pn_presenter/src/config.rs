//! Presenter configuration (`docs/04-l2-presenter.md` §5): the INI model lives in
//! `pn_shared` (also used by `PN-Patch.exe`); this module reads it once and caches it.
//!
//! Every skipped INI line is logged: a typo in someone's INI must not take the presenter
//! down ([`DEFAULT`] is a working presenter).

use std::sync::OnceLock;

pub use pn_shared::{Config, DEFAULT, Filter, Mode, Scale};

static CURRENT: OnceLock<Config> = OnceLock::new();

pub fn current() -> Config {
    CURRENT.get().copied().unwrap_or(DEFAULT)
}

pub fn enabled() -> bool {
    current().enabled
}

/// Reads the INI once, at load, and caches the result.
pub fn load() {
    let path = pn_shared::existing_path(crate::win::game_dir().as_deref());
    let text = path
        .as_ref()
        .map_or_else(String::new, |p| match std::fs::read(p) {
            Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| {
                // Review s12/s13 P1-1: a UTF-16 INI used to read as "no file" with no trace.
                crate::log::line(
                    "config",
                    format_args!("{} is not UTF-8: ignored", p.display()),
                );
                String::new()
            }),
            Err(e) => {
                crate::log::line("config", format_args!("{} unreadable: {e}", p.display()));
                String::new()
            }
        });
    let (cfg, bad) = pn_shared::parse(&text);
    for msg in bad {
        crate::log::line("config", format_args!("{msg}"));
    }
    crate::log::line("config", format_args!("file={path:?} {cfg:?}"));
    let _ = CURRENT.set(cfg);
}
