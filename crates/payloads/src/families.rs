//! One variable at a time, each body about 16 KiB unless the variable is its
//! size or depth, the body is another workload's value written differently
//! (`pretty-json-large`), or a fixed set of values (`ints-extremes`).
//!
//! `floats-short`, `sparse`, `defaults` and `keys-long-int` are the shapes
//! real API bodies are made of rather than one property pushed to its limit:
//! prices and coordinates, mostly-null records, omitted fields filled by
//! `#[serde(default)]`, and long or integer map keys.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::kynos::{Catalog, CatalogRef, JsonLarge};
use crate::{Group, Rng, Workload, encode_canonical, encode_pretty};

/// The target size of a family body.
pub const FAMILY_BYTES: usize = 16 * 1024;

/// Grow a vector with `next` until its encoding reaches [`FAMILY_BYTES`].
fn fill<T: Serialize>(next: impl FnMut() -> T) -> Vec<T> {
    fill_to(FAMILY_BYTES, next)
}

/// Grow a vector with `next` until its encoding reaches `target` bytes.
pub(crate) fn fill_to<T: Serialize>(target: usize, mut next: impl FnMut() -> T) -> Vec<T> {
    let mut values = Vec::new();
    let mut size = 2;
    while size < target {
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
    /// Signed values across every digit count up to `i64::MAX`'s 19.
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
        // At 19 digits the bound is 2^63, not 10^19, so the magnitude stays an
        // `i64` and negates without overflow.
        let signed = (0..700)
            .map(|_| {
                let digits = rng.below(20) as u32;
                let magnitude = rng.below(10u64.pow(digits).clamp(10, 1 << 63)) as i64;
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

/// Integers at every digit count's boundaries.
#[derive(Debug)]
pub struct IntsExtremes;

impl Workload for IntsExtremes {
    const NAME: &'static str = "ints-extremes";
    const GROUP: Group = Group::Family;
    type Owned = Integers;
    type Borrowed<'de> = Integers;

    /// For each digit count (1..=20 unsigned, 1..=19 signed, both signs) its
    /// smallest and largest value and one drawn between them, then the type
    /// limits, `0` and `-1`.
    fn value() -> Integers {
        let mut rng = Rng::new(0xe7e3);
        let mut unsigned = Vec::new();
        for digits in 1..=20 {
            let low = if digits == 1 { 0 } else { 10u64.pow(digits - 1) };
            let high = 10u64.checked_pow(digits).map_or(u64::MAX, |bound| bound - 1);
            unsigned.extend([low, high, low + rng.below(high - low)]);
        }
        unsigned.extend([0, u64::MAX]);
        let mut signed = Vec::new();
        for digits in 1..=19 {
            let low = if digits == 1 { 0 } else { 10i64.pow(digits - 1) };
            let high = 10i64.checked_pow(digits).map_or(i64::MAX, |bound| bound - 1);
            let drawn = low + rng.below((high - low) as u64) as i64;
            signed.extend([low, high, drawn, -low, -high, -drawn]);
        }
        signed.extend([i64::MIN, i64::MAX, 0, -1]);
        Integers { unsigned, signed }
    }
}

/// `f32`s drawn uniformly over finite bit patterns.
#[derive(Debug)]
pub struct F32s;

impl Workload for F32s {
    const NAME: &'static str = "f32s";
    const GROUP: Group = Group::Family;
    type Owned = Vec<f32>;
    type Borrowed<'de> = Vec<f32>;

    fn value() -> Vec<f32> {
        let mut rng = Rng::new(0xf32);
        fill(|| rng.finite_f32())
    }
}

/// Length of the `long-string` string.
pub const LONG_STRING_BYTES: usize = 256 * 1024;

/// One string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text {
    /// The string.
    pub text: String,
}

/// [`Text`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRef<'a> {
    /// The string.
    #[serde(borrow)]
    pub text: Cow<'a, str>,
}

/// One 256 KiB string of printable ASCII with nothing to escape, so the
/// string scanner's throughput is all there is to measure.
#[derive(Debug)]
pub struct LongString;

impl Workload for LongString {
    const NAME: &'static str = "long-string";
    const GROUP: Group = Group::Family;
    type Owned = Text;
    type Borrowed<'de> = TextRef<'de>;

    fn value() -> Text {
        let mut rng = Rng::new(0x5791);
        let printable: Vec<char> = (b' '..=b'~')
            .filter(|b| !matches!(b, b'"' | b'\\'))
            .map(char::from)
            .collect();
        Text {
            text: (0..LONG_STRING_BYTES).map(|_| *rng.pick(&printable)).collect(),
        }
    }
}

/// `json-large`'s value written by `to_vec_pretty`: the one variable is the
/// whitespace between tokens.
#[derive(Debug)]
pub struct PrettyJsonLarge;

impl Workload for PrettyJsonLarge {
    const NAME: &'static str = "pretty-json-large";
    const GROUP: Group = Group::Family;
    type Owned = Catalog;
    type Borrowed<'de> = CatalogRef<'de>;

    fn value() -> Catalog {
        JsonLarge::value()
    }

    fn bytes() -> Vec<u8> {
        encode_pretty(&Self::value())
    }
}

/// A price and a position, as the shortest decimal text of their values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    /// A price in a currency unit, to the cent.
    pub price: f64,
    /// Latitude in degrees, to six decimals.
    pub lat: f64,
    /// Longitude in degrees, to six decimals.
    pub lon: f64,
}

/// A whole number of millionths of a degree in `-degrees..=degrees`, as
/// degrees.
fn microdegrees(rng: &mut Rng, degrees: i64) -> f64 {
    let span = degrees * 1_000_000;
    (rng.below(2 * span as u64 + 1) as i64 - span) as f64 / 1e6
}

/// The commonest real-world floats: prices with two decimals and coordinates
/// with six. Each value is an integer count of hundredths or millionths
/// divided once, so it is the double nearest that decimal and serde_json's
/// shortest representation writes it back as that decimal (`12.5` for
/// `12.50`): at most 2 and 6 fractional digits, unlike `floats`' 17.
#[derive(Debug)]
pub struct FloatsShort;

impl Workload for FloatsShort {
    const NAME: &'static str = "floats-short";
    const GROUP: Group = Group::Family;
    type Owned = Vec<Quote>;
    type Borrowed<'de> = Vec<Quote>;

    fn value() -> Vec<Quote> {
        let mut rng = Rng::new(0x5407);
        fill(|| Quote {
            price: (1 + rng.below(1_000_000)) as f64 / 100.0,
            lat: microdegrees(&mut rng, 90),
            lon: microdegrees(&mut rng, 180),
        })
    }
}

/// A sparse API record: optional members mostly `null`, flags, and
/// collections mostly empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// An optional number.
    pub a: Option<u64>,
    /// An optional string.
    pub b: Option<String>,
    /// Feature flags.
    pub flags: [bool; 4],
    /// Labels, mostly none.
    pub tags: Vec<String>,
    /// Attributes, mostly none.
    pub meta: BTreeMap<String, String>,
}

/// [`Profile`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileRef<'a> {
    /// An optional number.
    pub a: Option<u64>,
    /// An optional string.
    #[serde(borrow)]
    pub b: Option<Cow<'a, str>>,
    /// Feature flags.
    pub flags: [bool; 4],
    /// Labels, mostly none.
    #[serde(borrow)]
    pub tags: Vec<Cow<'a, str>>,
    /// Attributes, mostly none.
    #[serde(borrow)]
    pub meta: BTreeMap<Cow<'a, str>, Cow<'a, str>>,
}

/// Records dominated by `null`, booleans, `[]` and `{}`: each optional member
/// and each collection is present one time in eight.
#[derive(Debug)]
pub struct Sparse;

impl Workload for Sparse {
    const NAME: &'static str = "sparse";
    const GROUP: Group = Group::Family;
    type Owned = Vec<Profile>;
    type Borrowed<'de> = Vec<ProfileRef<'de>>;

    fn value() -> Vec<Profile> {
        let mut rng = Rng::new(0x5ba45e);
        fill(|| {
            let [a, b, tags, meta] = [0; 4].map(|_| rng.below(8) == 0);
            Profile {
                a: a.then(|| rng.below(1 << 32)),
                b: b.then(|| format!("user-{}", rng.below(100_000))),
                flags: [0; 4].map(|_| rng.below(2) == 0),
                tags: if tags {
                    (0..=rng.below(2)).map(|_| format!("tag-{}", rng.below(50))).collect()
                } else {
                    Vec::new()
                },
                meta: if meta {
                    BTreeMap::from([(format!("k{}", rng.below(10)), format!("v{}", rng.below(1000)))])
                } else {
                    BTreeMap::new()
                },
            }
        })
    }
}

/// A settings record of ten members, six of them `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    /// Identifier, always present.
    pub id: u64,
    /// Name, always present.
    pub name: String,
    /// Kind, always present.
    pub kind: String,
    /// Revision, always present.
    pub version: u32,
    /// Free text, usually omitted.
    #[serde(default)]
    pub description: String,
    /// Usually omitted.
    #[serde(default)]
    pub enabled: bool,
    /// Usually omitted.
    #[serde(default)]
    pub retries: u32,
    /// Usually omitted.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Usually omitted.
    #[serde(default)]
    pub parent: Option<u64>,
    /// Usually omitted.
    #[serde(default)]
    pub owner: Option<String>,
}

/// [`Setting`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingRef<'a> {
    /// Identifier, always present.
    pub id: u64,
    /// Name, always present.
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    /// Kind, always present.
    #[serde(borrow)]
    pub kind: Cow<'a, str>,
    /// Revision, always present.
    pub version: u32,
    /// Free text, usually omitted.
    #[serde(default, borrow)]
    pub description: Cow<'a, str>,
    /// Usually omitted.
    #[serde(default)]
    pub enabled: bool,
    /// Usually omitted.
    #[serde(default)]
    pub retries: u32,
    /// Usually omitted.
    #[serde(default, borrow)]
    pub labels: Vec<Cow<'a, str>>,
    /// Usually omitted.
    #[serde(default)]
    pub parent: Option<u64>,
    /// Usually omitted.
    #[serde(default, borrow)]
    pub owner: Option<Cow<'a, str>>,
}

/// [`Setting`] as it arrives: a defaulted member at its default is omitted.
#[derive(Serialize)]
struct SettingWire<'a> {
    id: u64,
    name: &'a str,
    kind: &'a str,
    version: u32,
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    enabled: bool,
    #[serde(skip_serializing_if = "is_zero")]
    retries: u32,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    labels: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'a str>,
}

impl<'a> SettingWire<'a> {
    fn of(setting: &'a Setting) -> Self {
        Self {
            id: setting.id,
            name: &setting.name,
            kind: &setting.kind,
            version: setting.version,
            description: &setting.description,
            enabled: setting.enabled,
            retries: setting.retries,
            labels: &setting.labels,
            parent: setting.parent,
            owner: setting.owner.as_deref(),
        }
    }
}

#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde passes a reference")]
fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Kinds of [`Setting`].
const KINDS: &[&str] = &["flag", "limit", "route", "quota", "secret"];

/// Records that omit about half their members: each of [`Setting`]'s six
/// defaulted members is present one time in six, and present only with a
/// value other than its default, so the omitted members are exactly those
/// the decoder fills. The value is what decoding yields, defaults filled;
/// an encoder writes every member, so its output is longer than the input.
#[derive(Debug)]
pub struct Defaults;

impl Workload for Defaults {
    const NAME: &'static str = "defaults";
    const GROUP: Group = Group::Family;
    type Owned = Vec<Setting>;
    type Borrowed<'de> = Vec<SettingRef<'de>>;

    fn value() -> Vec<Setting> {
        let mut rng = Rng::new(0xdefa);
        let mut values: Vec<Setting> = Vec::new();
        // Filled to the size of the bytes, not of the value's encoding.
        let mut size = 2;
        while size < FAMILY_BYTES {
            let id = values.len() as u64 + 1;
            let present = [0; 6].map(|_| rng.below(6) == 0);
            let setting = Setting {
                id,
                name: format!("setting-{id:05}"),
                kind: (*rng.pick(KINDS)).to_owned(),
                version: 1 + rng.below(20) as u32,
                description: if present[0] {
                    format!("applies to {} requests", rng.below(1000))
                } else {
                    String::new()
                },
                enabled: present[1],
                retries: if present[2] { 1 + rng.below(9) as u32 } else { 0 },
                labels: if present[3] {
                    vec![format!("team-{}", rng.below(30))]
                } else {
                    Vec::new()
                },
                parent: present[4].then(|| rng.below(id)),
                owner: present[5].then(|| format!("owner-{}", rng.below(100))),
            };
            size += encode_canonical(&SettingWire::of(&setting)).len() + 1;
            values.push(setting);
        }
        values
    }

    fn bytes() -> Vec<u8> {
        encode_canonical(&Self::value().iter().map(SettingWire::of).collect::<Vec<_>>())
    }
}

/// Maps keyed two ways a typed body meets: long string keys, and integer
/// keys, which JSON writes as strings holding the integer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyed {
    /// 64-byte keys.
    pub long: BTreeMap<String, u32>,
    /// Integer keys.
    pub by_id: BTreeMap<u64, u32>,
}

/// [`Keyed`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyedRef<'a> {
    /// 64-byte keys.
    #[serde(borrow)]
    pub long: BTreeMap<Cow<'a, str>, u32>,
    /// Integer keys.
    pub by_id: BTreeMap<u64, u32>,
}

/// Length of a `keys-long-int` long key.
pub const LONG_KEY_BYTES: usize = 64;

/// A map of 64-byte keys and a map of integer keys across every digit
/// count, about half the body each.
#[derive(Debug)]
pub struct KeysLongInt;

impl Workload for KeysLongInt {
    const NAME: &'static str = "keys-long-int";
    const GROUP: Group = Group::Family;
    type Owned = Keyed;
    type Borrowed<'de> = KeyedRef<'de>;

    fn value() -> Keyed {
        let mut rng = Rng::new(0x10c6);
        let alphabet: Vec<char> = ('a'..='z').chain(['.', '_']).collect();
        let mut keyed = Keyed {
            long: BTreeMap::new(),
            by_id: BTreeMap::new(),
        };
        // A long member is about three integer-keyed members' bytes.
        let mut size = r#"{"long":{},"by_id":{}}"#.len();
        while size < FAMILY_BYTES {
            let prefix = format!("service.{:04}.", keyed.long.len());
            let rest: String = (prefix.len()..LONG_KEY_BYTES).map(|_| *rng.pick(&alphabet)).collect();
            let value = rng.below(1 << 32) as u32;
            size += LONG_KEY_BYTES + value.to_string().len() + 4;
            keyed.long.insert(prefix + &rest, value);
            for _ in 0..3 {
                let digits = 1 + rng.below(20) as u32;
                let low = if digits == 1 { 0 } else { 10u64.pow(digits - 1) };
                let high = 10u64.checked_pow(digits).map_or(u64::MAX, |bound| bound - 1);
                let id = low + rng.below(high - low + 1);
                let value = rng.below(1 << 32) as u32;
                if keyed.by_id.insert(id, value).is_none() {
                    size += id.to_string().len() + value.to_string().len() + 4;
                }
            }
        }
        keyed
    }
}
