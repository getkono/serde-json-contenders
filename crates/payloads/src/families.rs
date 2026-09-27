//! One variable at a time, each body about 16 KiB unless the variable is its
//! size or depth.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Group, Rng, Workload, encode_canonical};

/// The target size of a family body.
pub const FAMILY_BYTES: usize = 16 * 1024;

/// Grow a vector with `next` until its encoding reaches [`FAMILY_BYTES`].
fn fill<T: Serialize>(mut next: impl FnMut() -> T) -> Vec<T> {
    let mut values = Vec::new();
    let mut size = 2;
    while size < FAMILY_BYTES {
        let value = next();
        size += encode_canonical(&value).len() + 1;
        values.push(value);
    }
    values
}

/// Integers across every digit count, signed and unsigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Integers {
    /// Unsigned values across every digit count up to `u64::MAX`'s 20.
    pub unsigned: Vec<u64>,
    /// Signed values across every digit count up to 19.
    pub signed: Vec<i64>,
}

/// `ints`.
#[derive(Debug)]
pub struct Ints;

impl Workload for Ints {
    const NAME: &'static str = "ints";
    const GROUP: Group = Group::Family;
    type Owned = Integers;
    type Borrowed<'de> = Integers;

    fn value() -> Integers {
        let mut rng = Rng::new(0x1175);
        let unsigned = (0..700)
            .map(|_| {
                let digits = rng.below(20) as u32;
                rng.below(10u64.saturating_pow(digits).max(10)) | u64::from(digits == 19) << 63
            })
            .collect();
        let signed = (0..700)
            .map(|_| {
                let digits = rng.below(19) as u32;
                let magnitude = rng.below(10u64.pow(digits).max(10)) as i64;
                if rng.below(2) == 0 { -magnitude } else { magnitude }
            })
            .collect();
        Integers { unsigned, signed }
    }
}

/// Floats drawn uniformly over finite bit patterns.
#[derive(Debug)]
pub struct Floats;

impl Workload for Floats {
    const NAME: &'static str = "floats";
    const GROUP: Group = Group::Family;
    type Owned = Vec<f64>;
    type Borrowed<'de> = Vec<f64>;

    fn value() -> Vec<f64> {
        let mut rng = Rng::new(0xf10a7);
        fill(|| rng.finite_f64())
    }
}

/// Characters that must be escaped, dense enough that about a quarter of
/// string bytes are escapes.
const ESCAPED: &[char] = &['"', '\\', '\n', '\t', '\r', '\u{8}', '\u{c}', '\u{1}', '\u{1f}'];

/// Strings with dense escapes.
#[derive(Debug)]
pub struct Escapes;

impl Workload for Escapes {
    const NAME: &'static str = "escapes";
    const GROUP: Group = Group::Family;
    type Owned = Vec<String>;
    type Borrowed<'de> = Vec<Cow<'de, str>>;

    fn value() -> Vec<String> {
        let mut rng = Rng::new(0xe5c);
        fill(|| {
            (0..32)
                .map(|_| {
                    if rng.below(8) == 0 {
                        *rng.pick(ESCAPED)
                    } else {
                        char::from(b'a' + rng.below(26) as u8)
                    }
                })
                .collect::<String>()
        })
    }
}

/// Non-ASCII ranges: CJK, Hangul, Cyrillic, Greek, emoji.
const RANGES: &[(u32, u32)] = &[
    (0x4e00, 0x9fff),
    (0xac00, 0xd7a3),
    (0x0400, 0x04ff),
    (0x0370, 0x03ff),
    (0x1f300, 0x1f5ff),
];

/// Strings dominated by non-ASCII characters, written raw.
#[derive(Debug)]
pub struct Unicode;

impl Workload for Unicode {
    const NAME: &'static str = "unicode";
    const GROUP: Group = Group::Family;
    type Owned = Vec<String>;
    type Borrowed<'de> = Vec<Cow<'de, str>>;

    fn value() -> Vec<String> {
        let mut rng = Rng::new(0x07f8);
        fill(|| {
            (0..16)
                .map(|_| {
                    if rng.below(10) == 0 {
                        ' '
                    } else {
                        let (lo, hi) = *rng.pick(RANGES);
                        char::from_u32(lo + rng.below(u64::from(hi - lo + 1)) as u32).unwrap_or('?')
                    }
                })
                .collect::<String>()
        })
    }
}

/// One level of an object nesting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Depth from the root, so levels are distinguishable.
    pub depth: u32,
    /// The next level, absent at the leaf.
    pub next: Option<Box<Node>>,
}

impl Drop for Node {
    /// Iterative, so dropping a deep chain cannot overflow the stack and be
    /// mistaken for a parser's overflow.
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut node) = next {
            next = node.next.take();
        }
    }
}

/// Objects nested `D` deep. serde_json's limit is 128, so 127 and 129
/// straddle it; 10 000 is past every bounded parser.
#[derive(Debug)]
pub struct Nested<const D: u32>;

impl<const D: u32> Workload for Nested<D> {
    const NAME: &'static str = match D {
        100 => "nested-100",
        127 => "nested-127",
        129 => "nested-129",
        10_000 => "nested-10000",
        _ => panic!("unnamed nesting depth"),
    };
    const GROUP: Group = Group::Family;
    type Owned = Node;
    type Borrowed<'de> = Node;

    fn value() -> Node {
        let mut node = Node {
            depth: D - 1,
            next: None,
        };
        for depth in (0..D - 1).rev() {
            node = Node {
                depth,
                next: Some(Box::new(node)),
            };
        }
        node
    }

    /// Written directly rather than serialized: linear, and no recursion that
    /// could itself overflow at 10 000 levels.
    fn bytes() -> Vec<u8> {
        let mut out = String::new();
        for depth in 0..D {
            out.push_str(&format!("{{\"depth\":{depth},\"next\":"));
        }
        out.push_str("null");
        out.push_str(&"}".repeat(D as usize));
        out.into_bytes()
    }
}

/// An object with 1 000 keys.
#[derive(Debug)]
pub struct Wide;

impl Workload for Wide {
    const NAME: &'static str = "wide-1000";
    const GROUP: Group = Group::Family;
    type Owned = BTreeMap<String, u64>;
    type Borrowed<'de> = BTreeMap<Cow<'de, str>, u64>;

    fn value() -> BTreeMap<String, u64> {
        let mut rng = Rng::new(0x3a1de);
        (0..1000)
            .map(|i| (format!("key_{i:04}_{:x}", rng.below(1 << 20)), rng.below(1 << 32)))
            .collect()
    }
}

/// A small struct repeated many times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    /// Abscissa.
    pub x: i32,
    /// Ordinate.
    pub y: i32,
}

/// A long array of small structs.
#[derive(Debug)]
pub struct Points;

impl Workload for Points {
    const NAME: &'static str = "array-small";
    const GROUP: Group = Group::Family;
    type Owned = Vec<Point>;
    type Borrowed<'de> = Vec<Point>;

    fn value() -> Vec<Point> {
        let mut rng = Rng::new(0x9017);
        fill(|| Point {
            x: rng.below(20_001) as i32 - 10_000,
            y: rng.below(20_001) as i32 - 10_000,
        })
    }
}
