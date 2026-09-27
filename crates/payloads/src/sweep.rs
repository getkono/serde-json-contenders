//! The size sweep: the same record shape at eight body sizes.
//!
//! The point is the crossover below which a SIMD parser's setup costs more
//! than it saves. Each body is exactly `N` bytes: `N / 100` records (at least
//! one), with the first record's tags dropped and its name padded to make up the difference.

use crate::kynos::{Item, ItemRef, item};
use crate::{Group, Workload, encode_canonical};

/// A `Vec<Item>` encoding to exactly `N` bytes.
#[derive(Debug)]
pub struct Sweep<const N: usize>;

/// The records of a sweep body of `target` bytes.
///
/// # Panics
/// If `target` is below the smallest body one record can make.
#[must_use]
pub fn records(target: usize) -> Vec<Item> {
    let count = (target / 100).max(1) as u64;
    let mut items: Vec<Item> = (0..count).map(item).collect();
    items[0].name.clear();
    items[0].tags.clear();
    let base = encode_canonical(&items).len();
    assert!(base <= target, "sweep target {target} is below the {base}-byte floor");
    items[0].name = "x".repeat(target - base);
    items
}

impl<const N: usize> Workload for Sweep<N> {
    const NAME: &'static str = match N {
        64 => "sweep-64b",
        256 => "sweep-256b",
        1024 => "sweep-1kib",
        4096 => "sweep-4kib",
        16384 => "sweep-16kib",
        65536 => "sweep-64kib",
        262_144 => "sweep-256kib",
        1_048_576 => "sweep-1mib",
        _ => panic!("unnamed sweep size"),
    };
    const GROUP: Group = Group::Sweep;
    type Owned = Vec<Item>;
    type Borrowed<'de> = Vec<ItemRef<'de>>;

    fn value() -> Vec<Item> {
        records(N)
    }
}
