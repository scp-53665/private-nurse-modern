//! Diagnostic logging: one text log, one CSV of present observations, both in a directory
//! that the run recipe can point at (`PN_LOG_DIR`, default `%TEMP%\pn_modern`).
//!
//! Every write goes through `Mutex`, and a poison error is recovered rather than
//! propagated: a panicking hook must not turn into a silent game freeze.

use std::fmt::Arguments;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

#[derive(Debug)]
pub struct Config {
    pub dir: PathBuf,
    pub max_frames: u32,
    pub dump: bool,
}

static SPIKE: AtomicBool = AtomicBool::new(false);
static CONFIG: OnceLock<Config> = OnceLock::new();
static LOG: OnceLock<Mutex<File>> = OnceLock::new();
static CSV: OnceLock<Mutex<File>> = OnceLock::new();
static T0: OnceLock<Instant> = OnceLock::new();

/// True when the run asked for the Phase A observer class (`PN_LOG_DIR` set).
///
/// A shipped run writes a small diagnostic log and nothing else: no frame dumps, no CSV,
/// no per-blit pixel read-back (NF-14/NF-15) — and none of the counted observers record
/// ([`crate::count`]).
pub fn spike() -> bool {
    SPIKE.load(Ordering::Relaxed)
}

/// Reads the knobs and opens `pn_modern.log` + `frames.csv`. Failure to open leaves the
/// spike silent (and therefore useless) — the run recipe treats a missing log as a
/// failed run, not as "nothing happened".
pub fn init() -> std::io::Result<()> {
    let explicit = std::env::var_os("PN_LOG_DIR");
    SPIKE.store(explicit.is_some(), Ordering::Relaxed);
    let dir = explicit.map_or_else(
        || {
            let temp = std::env::var_os("TEMP").map_or_else(|| PathBuf::from("."), PathBuf::from);
            temp.join("pn_modern")
        },
        PathBuf::from,
    );
    let max_frames = std::env::var("PN_LOG_MAXFRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(240);
    let dump = std::env::var("PN_LOG_NODUMP").map_or(true, |v| v != "1");
    std::fs::create_dir_all(&dir)?;
    if !spike() {
        rotate(&dir.join("pn_modern.log"));
        rotate(&dir.join("frames.csv"));
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("pn_modern.log"))?;
    let csv = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("frames.csv"))?;
    let _ = T0.set(Instant::now());
    let _ = LOG.set(Mutex::new(log));
    let _ = CSV.set(Mutex::new(csv));
    let _ = CONFIG.set(Config {
        dir,
        max_frames,
        dump,
    });
    Ok(())
}

/// Record Q9: a normal run's log appends across sessions; past 1 MiB it moves to
/// `<name>.1` (replacing the previous one), so at most ~2 MiB stay on disk.
fn rotate(path: &std::path::Path) {
    const ROTATE_BYTES: u64 = 1 << 20;
    if std::fs::metadata(path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        // Best effort: a failed rotation only means a longer log.
        let _ = std::fs::rename(path, old);
    }
}

pub fn dir() -> PathBuf {
    CONFIG
        .get()
        .map_or_else(|| PathBuf::from("."), |c| c.dir.clone())
}

pub fn max_frames() -> u32 {
    CONFIG.get().map_or(0, |c| c.max_frames)
}

pub fn dump_enabled() -> bool {
    CONFIG.get().is_some_and(|c| c.dump)
}

/// Milliseconds since DLL load; the time base for every time series in this spike.
pub fn ms() -> u64 {
    T0.get().map_or(0, |t| {
        // A spike log line is worth more than the truncation lint here.
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    })
}

fn lock(f: &'static OnceLock<Mutex<File>>) -> Option<MutexGuard<'static, File>> {
    let cell = f.get()?;
    Some(
        cell.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Appends one `[t_ms] tag text` line. Write errors are dropped on purpose: a full
/// disk must not stall the game.
pub fn line(tag: &str, args: Arguments<'_>) {
    if let Some(mut f) = lock(&LOG) {
        let _ = writeln!(f, "[{}] {tag} {args}", ms());
    }
}

/// Appends one CSV row (no newline in `args`).
pub fn csv(row: Arguments<'_>) {
    if let Some(mut f) = lock(&CSV) {
        let _ = writeln!(f, "{row}");
    }
}

/// Path of a frame dump file, creating `frames/` on first use.
pub fn frame_path(name: &str) -> Option<PathBuf> {
    let d = dir().join("frames");
    std::fs::create_dir_all(&d).ok()?;
    Some(d.join(name))
}
