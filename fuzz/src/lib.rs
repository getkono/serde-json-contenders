//! What a divergence is, shared by every target, and which backend a run
//! fuzzes.
//!
//! A build compiles serde_json and one candidate crate, which may provide
//! more than one backend (simd-json with and without reused buffers, flexon
//! from a shared slice and in place). Each run fuzzes one of them, named by
//! `FUZZ_BACKEND`; unset, it is the first backend after the reference, and
//! with no candidate crate compiled, serde_json against itself (the
//! harness's own check). A divergence panics, which libFuzzer records as a
//! crash with its input.

use std::marker::PhantomData;
use std::sync::OnceLock;

use codecs::{Backend, Visit};
use serde_json::Value;

/// The environment variable naming the backend a run fuzzes.
pub const BACKEND_VAR: &str = "FUZZ_BACKEND";

/// The backend this run fuzzes, resolved once.
///
/// # Panics
///
/// If `FUZZ_BACKEND` names a backend this build did not compile, so a
/// misconfigured run fails at its first input rather than fuzzing another
/// backend under that name.
#[must_use]
pub fn candidate() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let compiled = codecs::compiled();
        match std::env::var(BACKEND_VAR) {
            Ok(name) => compiled
                .iter()
                .copied()
                .find(|c| *c == name)
                .unwrap_or_else(|| panic!("{BACKEND_VAR}={name} is not compiled into this build: {compiled:?}")),
            Err(_) => compiled.get(1).or(compiled.first()).copied().unwrap_or("serde_json"),
        }
    })
}

/// One target's check, generic over the backend it runs against.
pub trait Check {
    /// Check `data` against `B`, panicking on a divergence.
    fn check<B: Backend>(data: &[u8]);
}

struct Run<'a, C>(&'a [u8], PhantomData<C>);

impl<C: Check> Visit for Run<'_, C> {
    type Output = ();
    fn visit<B: Backend>(self) {
        C::check::<B>(self.0);
    }
}

/// Run `C`'s check on `data` against [`candidate`].
pub fn run<C: Check>(data: &[u8]) {
    codecs::dispatch(candidate(), Run::<C>(data, PhantomData));
}

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
