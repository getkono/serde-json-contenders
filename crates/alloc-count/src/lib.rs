//! A global allocator that counts, around the system allocator.
//!
//! Install it with `#[global_allocator]` in a binary that measures, never in a
//! library: a global allocator is process-wide.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// The counting allocator.
#[derive(Debug)]
pub struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static REALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

fn grow(by: u64) {
    let live = LIVE.fetch_add(by, Relaxed) + by;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged; the counters are side effects that allocate nothing.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        BYTES.fetch_add(layout.size() as u64, Relaxed);
        grow(layout.size() as u64);
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        BYTES.fetch_add(layout.size() as u64, Relaxed);
        grow(layout.size() as u64);
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as u64, Relaxed);
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REALLOCATIONS.fetch_add(1, Relaxed);
        if new_size > layout.size() {
            let by = (new_size - layout.size()) as u64;
            BYTES.fetch_add(by, Relaxed);
            grow(by);
        } else {
            LIVE.fetch_sub((layout.size() - new_size) as u64, Relaxed);
        }
        // SAFETY: forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Counters at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// Fresh blocks (`alloc`, `alloc_zeroed`).
    pub allocations: u64,
    /// Resizes of an existing block.
    pub reallocations: u64,
    /// Bytes requested, counting only the growth of a reallocation.
    pub bytes: u64,
    /// Bytes live now.
    pub live: u64,
}

impl Snapshot {
    /// Read the counters.
    #[must_use]
    pub fn now() -> Self {
        Self {
            allocations: ALLOCATIONS.load(Relaxed),
            reallocations: REALLOCATIONS.load(Relaxed),
            bytes: BYTES.load(Relaxed),
            live: LIVE.load(Relaxed),
        }
    }
}

/// Reset the peak to the current live size, so the next [`peak`] reports the
/// high-water mark of what runs after this call.
pub fn reset_peak() {
    PEAK.store(LIVE.load(Relaxed), Relaxed);
}

/// The high-water mark of live bytes since the last [`reset_peak`].
#[must_use]
pub fn peak() -> u64 {
    PEAK.load(Relaxed)
}
