//! A per-thread counting global allocator for the unit-test binary, used by tests that
//! assert how many heap allocations an operation performs. Counters are thread-local, so
//! concurrently running tests do not observe each other.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

fn record_allocation() {
    ALLOCATIONS.with(|count| count.set(count.get() + 1));
}

// SAFETY: every method forwards to the system allocator unchanged; counting only touches a
// const-initialised thread-local cell, which neither allocates nor has a destructor.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Runs `f` and returns its result with the number of heap allocations (allocations and
/// reallocations) the current thread performed while it ran. Allocations made by `f`'s
/// return value being dropped later are not included.
pub(crate) fn count_allocations<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let before = ALLOCATIONS.with(Cell::get);
    let result = f();
    let after = ALLOCATIONS.with(Cell::get);
    (result, after - before)
}
