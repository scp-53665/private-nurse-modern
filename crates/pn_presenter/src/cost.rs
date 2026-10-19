//! Present-cost measurement (`docs/04-l2-presenter.md` §6 L2-M4: p99 present cost, the
//! escalation trigger is > ~15 ms per frame).
//!
//! Switched by `PN_COST=1` on its own, so it can run with every spike observer off
//! (`PN_LOG_DIR` unset): one QPC pair per presented blit into a fixed histogram, and a
//! summary line every 10 s from the census tick. Off, it costs one relaxed load.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::log;
use crate::win::{qpc_now, qpc_us_since};

/// 100 µs buckets up to 50 ms; the last bucket takes everything slower.
const BUCKET_US: u64 = 100;
const BUCKETS: usize = 501;

static HIST: [AtomicU32; BUCKETS] = [const { AtomicU32::new(0) }; BUCKETS];
static MAX_US: AtomicU64 = AtomicU64::new(0);
static TOTAL_US: AtomicU64 = AtomicU64::new(0);

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("PN_COST").is_ok_and(|v| v == "1"))
}

/// Times `f` (one present: composition + sink) when measuring.
pub fn timed<R>(f: impl FnOnce() -> R) -> R {
    if !enabled() {
        return f();
    }
    let t0 = qpc_now();
    let r = f();
    let us = qpc_us_since(t0);
    let i = usize::try_from(us / BUCKET_US)
        .unwrap_or(BUCKETS - 1)
        .min(BUCKETS - 1);
    HIST[i].fetch_add(1, Ordering::Relaxed);
    MAX_US.fetch_max(us, Ordering::Relaxed);
    TOTAL_US.fetch_add(us, Ordering::Relaxed);
    r
}

/// Logs the cumulative distribution (called from the 1 Hz tick; writes every 10th).
pub fn flush(tick: u32) {
    if !enabled() || tick % 10 != 9 {
        return;
    }
    let counts: Vec<u64> = HIST
        .iter()
        .map(|c| u64::from(c.load(Ordering::Relaxed)))
        .collect();
    let n: u64 = counts.iter().sum();
    if n == 0 {
        return;
    }
    let bound = |p: u64| {
        let (us, overflow) = percentile(&counts, p);
        format!("{}{us}", if overflow { ">" } else { "<=" })
    };
    log::line(
        "cost",
        format_args!(
            "presents={n} mean_us={} p50_us{} p99_us{} max_us={}",
            TOTAL_US.load(Ordering::Relaxed) / n,
            bound(50),
            bound(99),
            MAX_US.load(Ordering::Relaxed)
        ),
    );
}

/// Nearest-rank percentile of the bucket counts: the bucket's upper bound in µs, and
/// whether it is the overflow bucket (then the bound is a *lower* bound; review
/// s12/s13 P1-7).
fn percentile(counts: &[u64], p: u64) -> (u64, bool) {
    let n: u64 = counts.iter().sum();
    let want = (n * p).div_ceil(100);
    let mut seen = 0;
    for (i, c) in counts.iter().enumerate() {
        seen += c;
        if seen >= want {
            let overflow = i + 1 == counts.len();
            // the overflow bucket has no upper bound: report where it starts
            return ((i as u64 + u64::from(!overflow)) * BUCKET_US, overflow);
        }
    }
    ((counts.len() as u64 - 1) * BUCKET_US, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_bounds_and_overflow() {
        let mut c = vec![0u64; BUCKETS];
        c[0] = 98;
        c[2] = 1;
        c[BUCKETS - 1] = 1;
        assert_eq!(percentile(&c, 50), (BUCKET_US, false));
        assert_eq!(percentile(&c, 99), (3 * BUCKET_US, false)); // rank 99 of 100
        assert_eq!(
            percentile(&c, 100),
            ((BUCKETS as u64 - 1) * BUCKET_US, true)
        );
    }
}
