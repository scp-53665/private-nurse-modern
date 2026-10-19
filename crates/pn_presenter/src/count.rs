//! Aggregate counter for APIs that can be called in tight loops (input, geometry,
//! paint): count every call, keep a bounded sample of arguments.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Arguments kept for the first N calls; enough to see the coordinate spaces.
pub const SAMPLES: usize = 48;

pub struct Counted {
    pub label: &'static str,
    /// `u64`: the measured `IsIconic` poll rate wraps a `u32` counter in ~6 h.
    total: AtomicU64,
    /// Set by the first flush that took samples: a 10 kHz poll loop must not refill
    /// 48 samples every second (measured: 245 MB of log in one 180 s run).
    sampled_once: AtomicBool,
    /// Total at the previous flush: a counter that moved since is worth one log line
    /// (the acceptance runs need per-second present totals, not just first-call samples).
    logged: AtomicU64,
    samples: Mutex<Vec<[i64; 4]>>,
}

impl Counted {
    pub const fn new(label: &'static str) -> Self {
        Self {
            label,
            total: AtomicU64::new(0),
            sampled_once: AtomicBool::new(false),
            logged: AtomicU64::new(0),
            samples: Mutex::new(Vec::new()),
        }
    }

    /// One call: bump the counter, keep the first [`SAMPLES`] argument sets.
    ///
    /// Nothing is recorded outside a spike run: the shipped presenter must not spend
    /// cycles on observers whose log nobody asked for (NF-16): `args` (a clock read, a
    /// registry lookup) is only evaluated for a kept sample.
    pub fn note(&self, args: impl FnOnce() -> [i64; 4]) {
        if !crate::log::spike() {
            return;
        }
        self.total.fetch_add(1, Ordering::Relaxed);
        if self.sampled_once.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut s) = self.samples.lock()
            && s.len() < SAMPLES
        {
            s.push(args());
        }
    }

    /// Cumulative count and the samples collected since the last call (cleared).
    /// The second element of the first tuple is `false` when nothing changed since the
    /// previous flush — the caller can then stay quiet.
    pub fn take(&self) -> (u64, bool, Vec<[i64; 4]>) {
        let total = self.total.load(Ordering::Relaxed);
        let moved = total != self.logged.swap(total, Ordering::Relaxed);
        if !self.sampled_once.load(Ordering::Relaxed) && total != 0 {
            self.sampled_once.store(true, Ordering::Relaxed);
        }
        let samples = match self.samples.lock() {
            Ok(mut s) => std::mem::take(&mut *s),
            Err(p) => std::mem::take(&mut *p.into_inner()),
        };
        (total, moved, samples)
    }
}
