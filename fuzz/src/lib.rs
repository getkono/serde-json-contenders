//! What a divergence is, shared by every target.
//!
//! The candidate is the one backend this build compiled beside serde_json.
//! A divergence panics, which libFuzzer records as a crash with its input.

use serde_json::Value;

/// The candidate this build fuzzes.
#[cfg(feature = "sonic-rs")]
pub type Candidate = codecs::backend::sonic_rs::SonicRs;
/// The candidate this build fuzzes.
#[cfg(feature = "simd-json")]
pub type Candidate = codecs::backend::simd_json::SimdJson;
/// The candidate this build fuzzes.
#[cfg(any(feature = "flexon-rt", feature = "flexon-ct"))]
pub type Candidate = codecs::backend::flexon::Flexon;
/// The candidate this build fuzzes.
#[cfg(feature = "jiter")]
pub type Candidate = codecs::backend::jiter::Jiter;
/// The candidate this build fuzzes.
#[cfg(feature = "hifijson")]
pub type Candidate = codecs::backend::hifijson::Hifijson;
/// The candidate this build fuzzes.
#[cfg(feature = "struson")]
pub type Candidate = codecs::backend::struson::Struson;
/// With no candidate, serde_json against itself: the harness's own check.
#[cfg(not(any(
    feature = "sonic-rs",
    feature = "simd-json",
    feature = "flexon-rt",
    feature = "flexon-ct",
    feature = "jiter",
    feature = "hifijson",
    feature = "struson"
)))]
pub type Candidate = codecs::backend::serde_json::SerdeJson;

/// Floats this many units in the last place apart are a rounding difference,
/// which the conformance suite measures exactly; fuzzing hunts for the rest.
const ROUNDING_ULPS: u64 = 2;

fn ulps(a: f64, b: f64) -> u64 {
    let key = |x: f64| {
        let bits = x.to_bits() as i64;
        if bits < 0 { -(bits & i64::MAX) - 1 } else { bits }
    };
    key(a).abs_diff(key(b))
}

/// Whether two values are the same, floats up to rounding.
#[must_use]
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if x.is_f64() || y.is_f64() => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => x.to_bits() == y.to_bits() || ulps(x, y) <= ROUNDING_ULPS,
            _ => false,
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// The deepest nesting of arrays and objects in `data`, ignoring brackets
/// inside strings.
#[must_use]
pub fn depth(data: &[u8]) -> usize {
    let (mut depth, mut max, mut in_string, mut escaped) = (0usize, 0, false, false);
    for &b in data {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}
