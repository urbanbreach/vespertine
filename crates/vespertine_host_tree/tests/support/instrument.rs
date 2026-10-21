//! Test-support heap instrumentation: a counting global allocator.
//!
//! Install it in a test or example binary with
//! `#[global_allocator] static ALLOC: CountingAllocator = CountingAllocator;`.
//! Counters are per thread, so tests running concurrently on other threads do not disturb
//! a measurement. A `realloc` counts as one allocation of the new size and a release of
//! the old size.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
    static LIVE: Cell<i64> = const { Cell::new(0) };
    static PEAK: Cell<i64> = const { Cell::new(0) };
}

/// Allocation statistics of the current thread since the last [`reset`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocStats {
    /// Allocation calls (allocations, zeroed allocations and reallocations).
    pub count: u64,
    /// Total bytes requested by those calls.
    pub bytes: u64,
    /// Net bytes currently live relative to the reset point (may be negative when
    /// blocks allocated before the reset were freed).
    pub live_bytes: i64,
    /// Highest `live_bytes` reached since the reset: the peak transient allocation.
    pub peak_bytes: i64,
}

/// Starts a new measurement on the current thread.
pub fn reset() {
    COUNT.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    LIVE.with(|c| c.set(0));
    PEAK.with(|c| c.set(0));
}

/// Reads the statistics of the current thread.
pub fn snapshot() -> AllocStats {
    AllocStats {
        count: COUNT.with(Cell::get),
        bytes: BYTES.with(Cell::get),
        live_bytes: LIVE.with(Cell::get),
        peak_bytes: PEAK.with(Cell::get),
    }
}

fn record_alloc(size: usize) {
    let _ = COUNT.try_with(|c| c.set(c.get() + 1));
    let _ = BYTES.try_with(|c| c.set(c.get() + size as u64));
    let _ = LIVE.try_with(|live| {
        let now = live.get() + size as i64;
        live.set(now);
        let _ = PEAK.try_with(|peak| {
            if now > peak.get() {
                peak.set(now);
            }
        });
    });
}

fn record_free(size: usize) {
    let _ = LIVE.try_with(|live| live.set(live.get() - size as i64));
}

/// A global allocator that forwards to the system allocator and counts.
pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_alloc(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_alloc(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record_free(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_free(layout.size());
        record_alloc(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}
