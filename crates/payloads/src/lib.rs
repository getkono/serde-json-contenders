//! Every workload the comparison runs, as typed structs.
//!
//! A workload is a Rust type plus the one byte string that encodes it. The
//! bytes are a pure function of this crate: generators draw from [`Rng`], a
//! fixed splitmix64 stream, never from a clock or an RNG crate, so a
//! dependency bump cannot move an input. `tests` pins every corpus by SHA-256.
//!
//! Each workload has an owned form (`String` fields) and a borrowed form
//! (`#[serde(borrow)] Cow<'de, str>`). The borrowed form borrows wherever the
//! backend can hand out a slice of the input and allocates otherwise, so the
//! difference between the two forms *is* each backend's ability to borrow.

pub mod families;
pub mod kynos;
pub mod rng;
pub mod sweep;
pub mod world;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub use rng::Rng;

/// A workload: a typed value and the canonical bytes that encode it.
pub trait Workload: 'static {
    /// Stable identifier used in result files and on the command line.
    const NAME: &'static str;
    /// Which group the workload reports under.
    const GROUP: Group;
    /// The owned form.
    type Owned: Serialize + DeserializeOwned + std::fmt::Debug;
    /// The borrowed form.
    type Borrowed<'de>: Serialize + Deserialize<'de> + std::fmt::Debug;

    /// The value the bytes encode.
    #[must_use]
    fn value() -> Self::Owned;

    /// The canonical input bytes. serde_json's compact encoding unless a
    /// workload needs a byte shape serde_json never writes (such as `\u`
    /// escapes for non-ASCII).
    #[must_use]
    fn bytes() -> Vec<u8> {
        encode_canonical(&Self::value())
    }
}

/// Report grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// The three shapes the decision rule is evaluated on.
    Kynos,
    /// The size sweep.
    Sweep,
    /// One variable at a time.
    Family,
    /// Schema-equivalents of widely used real-world documents.
    World,
}

/// The canonical encoding: serde_json, compact.
///
/// # Panics
/// If a generated value does not serialize, which is a bug in this crate.
#[must_use]
pub fn encode_canonical<T: Serialize>(value: &T) -> Vec<u8> {
    match serde_json::to_vec(value) {
        Ok(bytes) => bytes,
        Err(error) => unreachable!("a generated value failed to serialize: {error}"),
    }
}

/// Rewrite every non-ASCII character as a `\u` escape (a surrogate pair above
/// U+FFFF). Valid because serde_json's output holds non-ASCII bytes only
/// inside strings.
#[must_use]
pub fn ascii_escape(json: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(json).unwrap_or_else(|_| unreachable!("serde_json writes UTF-8"));
    let mut out = String::with_capacity(json.len() * 2);
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out.into_bytes()
}

/// Receives a workload type chosen at run time.
pub trait Visit {
    /// What the visit produces.
    type Output;
    /// Called with the workload named on the command line.
    fn visit<W: Workload>(self) -> Self::Output;
}

/// Every workload, in report order.
pub const ALL: &[&str] = &[
    kynos::JsonSmall::NAME,
    kynos::EchoPost::NAME,
    kynos::JsonLarge::NAME,
    sweep::Sweep::<64>::NAME,
    sweep::Sweep::<256>::NAME,
    sweep::Sweep::<1024>::NAME,
    sweep::Sweep::<4096>::NAME,
    sweep::Sweep::<16384>::NAME,
    sweep::Sweep::<65536>::NAME,
    sweep::Sweep::<262_144>::NAME,
    sweep::Sweep::<1_048_576>::NAME,
    families::Ints::NAME,
    families::Floats::NAME,
    families::Escapes::NAME,
    families::Unicode::NAME,
    families::Nested::<100>::NAME,
    families::Nested::<127>::NAME,
    families::Nested::<129>::NAME,
    families::Nested::<10_000>::NAME,
    families::Wide::NAME,
    families::Points::NAME,
    world::TwitterLike::NAME,
    world::CitmLike::NAME,
    world::CanadaLike::NAME,
];

/// Dispatch `visitor` to the workload called `name`.
pub fn dispatch<V: Visit>(name: &str, visitor: V) -> Option<V::Output> {
    Some(match name {
        kynos::JsonSmall::NAME => visitor.visit::<kynos::JsonSmall>(),
        kynos::EchoPost::NAME => visitor.visit::<kynos::EchoPost>(),
        kynos::JsonLarge::NAME => visitor.visit::<kynos::JsonLarge>(),
        sweep::Sweep::<64>::NAME => visitor.visit::<sweep::Sweep<64>>(),
        sweep::Sweep::<256>::NAME => visitor.visit::<sweep::Sweep<256>>(),
        sweep::Sweep::<1024>::NAME => visitor.visit::<sweep::Sweep<1024>>(),
        sweep::Sweep::<4096>::NAME => visitor.visit::<sweep::Sweep<4096>>(),
        sweep::Sweep::<16384>::NAME => visitor.visit::<sweep::Sweep<16384>>(),
        sweep::Sweep::<65536>::NAME => visitor.visit::<sweep::Sweep<65536>>(),
        sweep::Sweep::<262_144>::NAME => visitor.visit::<sweep::Sweep<262_144>>(),
        sweep::Sweep::<1_048_576>::NAME => visitor.visit::<sweep::Sweep<1_048_576>>(),
        families::Ints::NAME => visitor.visit::<families::Ints>(),
        families::Floats::NAME => visitor.visit::<families::Floats>(),
        families::Escapes::NAME => visitor.visit::<families::Escapes>(),
        families::Unicode::NAME => visitor.visit::<families::Unicode>(),
        families::Nested::<100>::NAME => visitor.visit::<families::Nested<100>>(),
        families::Nested::<127>::NAME => visitor.visit::<families::Nested<127>>(),
        families::Nested::<129>::NAME => visitor.visit::<families::Nested<129>>(),
        families::Nested::<10_000>::NAME => visitor.visit::<families::Nested<10_000>>(),
        families::Wide::NAME => visitor.visit::<families::Wide>(),
        families::Points::NAME => visitor.visit::<families::Points>(),
        world::TwitterLike::NAME => visitor.visit::<world::TwitterLike>(),
        world::CitmLike::NAME => visitor.visit::<world::CitmLike>(),
        world::CanadaLike::NAME => visitor.visit::<world::CanadaLike>(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
