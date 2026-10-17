//! Modified for Unveil (2026-10-17): bounded, opt-in pool allocation/reuse counts by size and usage.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct Bucket {
    pub upper_bytes: u64,
    pub usage: u32,
    pub allocations: u64,
    pub reuses: u64,
    pub returns: u64,
    pub frees: u64,
}

#[derive(Clone, Debug, Default)]
pub struct PoolStats {
    pub buckets: Vec<Bucket>,
    pub allocated_peak: u64,
    pub pooled_peak: u64,
    pub incomplete: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Allocate,
    Reuse,
    Return,
    Free,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATS: Mutex<PoolStats> = Mutex::new(PoolStats { buckets: Vec::new(), allocated_peak: 0, pooled_peak: 0, incomplete: false });

pub fn set_pool_diagnostics(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn pool_stats(reset: bool) -> PoolStats {
    let mut stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
    let result = stats.clone();
    if reset {
        *stats = PoolStats::default();
    }
    result
}

pub(crate) fn record(size: u64, usage: u32, operation: Operation) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
    let memory = crate::memory();
    stats.allocated_peak = stats.allocated_peak.max(memory.allocated);
    stats.pooled_peak = stats.pooled_peak.max(memory.pooled);
    let upper = size.checked_next_power_of_two().unwrap_or(u64::MAX);
    let index = stats.buckets.iter().position(|b| b.upper_bytes == upper && b.usage == usage);
    let bucket = match index {
        Some(i) => &mut stats.buckets[i],
        None if stats.buckets.len() < 128 => {
            stats.buckets.push(Bucket { upper_bytes: upper, usage, ..Default::default() });
            let i = stats.buckets.len() - 1;
            &mut stats.buckets[i]
        }
        None => {
            stats.incomplete = true;
            return;
        }
    };
    match operation {
        Operation::Allocate => bucket.allocations += 1,
        Operation::Reuse => bucket.reuses += 1,
        Operation::Return => bucket.returns += 1,
        Operation::Free => bucket.frees += 1,
    }
}
