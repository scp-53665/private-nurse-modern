//! The presenter INI (`docs/04-l2-presenter.md` §5), shared by the DLL (reads it) and
//! `PN-Patch.exe` (reads and writes it), so the two can never disagree on a key.
//!
//! Location: `pn_modern.ini` next to `NURSEUS.EXE`, else
//! `%APPDATA%\PnModern\config.ini` (read-only install folders, e.g. Program Files).
//! A typo must not take the presenter down: bad lines are reported and skipped, and
//! [`DEFAULT`] is a working presenter.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// Largest integer scale the desktop holds (2x at >=1080p, 1x at <=720p).
    Auto,
    /// 1x..4x, clamped to what the desktop holds.
    Preset(u32),
    /// Largest scale that fits the desktop work area (fractional allowed).
    Fit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Windowed,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Nearest for integer scales, smooth otherwise (and nearest above a 1080p-class frame).
    Auto,
    Nearest,
    Smooth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Kill switch: 0 restores the engine's own behaviour (mode switch, no scaling).
    pub enabled: bool,
    pub scale: Scale,
    pub mode: Mode,
    pub filter: Filter,
    /// A2 (user decision, session 14): the DLL makes the process system-DPI-aware before
    /// the engine creates a window, so 150 % desktops render natively.
    pub dpi_aware: bool,
}

/// A game folder with no INI gets a working presenter: on, windowed, auto scale.
pub const DEFAULT: Config = Config {
    enabled: true,
    scale: Scale::Auto,
    mode: Mode::Windowed,
    filter: Filter::Auto,
    dpi_aware: true,
};

pub const PORTABLE_NAME: &str = "pn_modern.ini";

/// The INI next to the game.
#[must_use]
pub fn portable_path(game_dir: &Path) -> PathBuf {
    game_dir.join(PORTABLE_NAME)
}

/// The fallback INI for a read-only game folder (`None` without `%APPDATA%`).
#[must_use]
pub fn appdata_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("PnModern").join("config.ini"))
}

/// The INI the DLL reads: the portable one if it exists, else the `%APPDATA%` one.
#[must_use]
pub fn existing_path(game_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = game_dir.map(portable_path).filter(|p| p.exists()) {
        return Some(p);
    }
    appdata_path().filter(|p| p.exists())
}

fn flag(v: &str) -> Option<bool> {
    match v {
        "0" | "false" | "no" => Some(false),
        "1" | "true" | "yes" => Some(true),
        _ => None,
    }
}

/// `key=value` per line; `#`/`;` start a comment anywhere on a line (no value contains
/// them); `[sections]` tolerated and ignored.
/// Returns the config plus one message per line that was skipped.
#[must_use]
pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut c = DEFAULT;
    let mut bad = Vec::new();
    // A UTF-8 BOM (Notepad, PowerShell 5) would otherwise glue itself to the first key.
    let text = text.trim_start_matches('\u{feff}');
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or_default().trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bad.push(format!("ignored line {line:?}"));
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let v = value.trim().to_ascii_lowercase();
        let ok = match key.as_str() {
            "enabled" => set(&mut c.enabled, flag(&v)),
            "dpi_aware" => set(&mut c.dpi_aware, flag(&v)),
            "scale" => set(&mut c.scale, scale(&v)),
            "mode" => set(&mut c.mode, mode(&v)),
            "filter" => set(&mut c.filter, filter(&v)),
            _ => {
                bad.push(format!("unknown key {key:?}"));
                continue;
            }
        };
        if !ok {
            bad.push(format!("bad {key} {v:?}"));
        }
    }
    (c, bad)
}

/// A bad value keeps the previous good one.
fn set<T>(slot: &mut T, value: Option<T>) -> bool {
    let Some(v) = value else { return false };
    *slot = v;
    true
}

fn scale(v: &str) -> Option<Scale> {
    match v {
        "auto" => Some(Scale::Auto),
        "fit" => Some(Scale::Fit),
        n => match n.parse::<u32>() {
            Ok(n @ 1..=4) => Some(Scale::Preset(n)),
            _ => None,
        },
    }
}

fn mode(v: &str) -> Option<Mode> {
    match v {
        "windowed" => Some(Mode::Windowed),
        "fullscreen" => Some(Mode::Fullscreen),
        _ => None,
    }
}

fn filter(v: &str) -> Option<Filter> {
    match v {
        "auto" => Some(Filter::Auto),
        "nearest" => Some(Filter::Nearest),
        "smooth" => Some(Filter::Smooth),
        _ => None,
    }
}

/// The INI text for `c`; `parse(&to_ini(c)).0 == c` for every config.
#[must_use]
pub fn to_ini(c: &Config) -> String {
    let b = |v: bool| if v { "1" } else { "0" };
    let scale = match c.scale {
        Scale::Auto => "auto".to_string(),
        Scale::Fit => "fit".to_string(),
        Scale::Preset(n) => n.to_string(),
    };
    let mode = match c.mode {
        Mode::Windowed => "windowed",
        Mode::Fullscreen => "fullscreen",
    };
    let filter = match c.filter {
        Filter::Auto => "auto",
        Filter::Nearest => "nearest",
        Filter::Smooth => "smooth",
    };
    format!(
        "# Private Nurse — Modern Windows Patch settings (written by PN-Patch.exe)\n\
         enabled={}\nscale={scale}\nmode={mode}\nfilter={filter}\ndpi_aware={}\n",
        b(c.enabled),
        b(c.dpi_aware)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ini_is_the_default_presenter() {
        assert_eq!(parse("").0, DEFAULT);
        assert_eq!(parse("# comment\n; comment\n[presenter]\n").0, DEFAULT);
    }

    #[test]
    fn kill_switch_and_knobs() {
        let (c, bad) = parse("enabled=0\nscale=3\nmode=fullscreen\nfilter=smooth\n");
        assert!(bad.is_empty());
        assert!(!c.enabled);
        assert_eq!(c.scale, Scale::Preset(3));
        assert_eq!(c.mode, Mode::Fullscreen);
        assert_eq!(c.filter, Filter::Smooth);
        assert!(c.dpi_aware);
        assert!(!parse("dpi_aware=0").0.dpi_aware);
        assert_eq!(parse("scale = fit").0.scale, Scale::Fit);
        assert_eq!(parse("scale = auto").0.scale, Scale::Auto);
    }

    #[test]
    fn a_utf8_bom_does_not_defeat_the_kill_switch() {
        assert!(!parse("\u{feff}enabled=0\n").0.enabled);
    }

    #[test]
    fn the_readme_example_is_the_default_and_parses_cleanly() {
        let readme = include_str!("../../../README.md");
        let block = readme
            .split("```ini") // not "```ini\n": a CRLF checkout (core.autocrlf) has "\r\n"
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or_default();
        assert!(block.contains("dpi_aware"), "README ini block not found");
        let (c, bad) = parse(block);
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(c, DEFAULT);
    }

    #[test]
    fn inline_comments_as_in_the_readme() {
        let (c, bad) = parse("enabled=0   # off\nscale=2 ; two\nmode=fullscreen#x\n");
        assert!(bad.is_empty(), "{bad:?}");
        assert!(!c.enabled);
        assert_eq!(c.scale, Scale::Preset(2));
        assert_eq!(c.mode, Mode::Fullscreen);
    }

    #[test]
    fn bad_values_keep_the_previous_good_value_and_are_reported() {
        let (c, bad) = parse("scale=2\nscale=9\nmode=sideways\nscale=x\nfoo=1\nnoequals\n");
        assert_eq!(c.scale, Scale::Preset(2));
        assert_eq!(c.mode, Mode::Windowed);
        assert_eq!(bad.len(), 5, "{bad:?}");
    }

    #[test]
    fn to_ini_round_trips_every_config() {
        for enabled in [false, true] {
            for dpi_aware in [false, true] {
                for scale in [Scale::Auto, Scale::Fit, Scale::Preset(1), Scale::Preset(4)] {
                    for mode in [Mode::Windowed, Mode::Fullscreen] {
                        for filter in [Filter::Auto, Filter::Nearest, Filter::Smooth] {
                            let c = Config {
                                enabled,
                                scale,
                                mode,
                                filter,
                                dpi_aware,
                            };
                            let (back, bad) = parse(&to_ini(&c));
                            assert_eq!(back, c);
                            assert!(bad.is_empty(), "{bad:?}");
                        }
                    }
                }
            }
        }
    }
}
