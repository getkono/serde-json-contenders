//! Section `value_targets`: decoding into serde_json's own dynamic types.
//!
//! A server often takes `serde_json::Value` or `Map<String, Value>` as a
//! body. Their `Deserialize` impls are serde_json's, driven by the backend's
//! deserializer, so every `y_` file and `json-large` must decode to exactly
//! serde_json's result. Under `arbitrary_precision`, `Value`'s `Deserialize`
//! expects serde_json's private number token, which other backends never
//! send, so their numbers arrive as `u64`, `i64` or `f64` rather than as the
//! input's text. There, and only there, a disagreement that is nothing but
//! number representation (the same document, each number the same value or
//! the correctly rounded `f64` of serde_json's text) is recorded as a note;
//! every other disagreement gates in every build.

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use codecs::body::Arrival;
use payloads::Workload;
use payloads::kynos::JsonLarge;
use serde_json::Value;

use crate::corpus::{Corpus, Expect};
use crate::outcome::{AsMap, AsValue, Decode, Outcome, agree, borrowed, faithful, owned};
use crate::record::Section;

/// Whether number representation alone is excused in this build.
pub const NUMBERS_EXCUSED: bool = cfg!(feature = "arbitrary-precision");

/// Whether `backend` is `reference` but for how its numbers are held: the
/// same shape, strings, keys and literals, and each number either the same
/// value or the correctly rounded (finite) `f64` of the reference's text.
#[must_use]
pub fn same_but_numbers(reference: &Value, backend: &Value) -> bool {
    match (reference, backend) {
        (Value::Number(r), Value::Number(b)) => {
            let (r, b) = (r.to_string(), b.to_string());
            crate::exact::numbers_equal(&r, &b)
                || matches!(
                    (r.parse::<f64>(), b.parse::<f64>()),
                    (Ok(r), Ok(b)) if r.is_finite() && r.to_bits() == b.to_bits()
                )
        }
        (Value::Array(r), Value::Array(b)) => {
            r.len() == b.len() && r.iter().zip(b).all(|(r, b)| same_but_numbers(r, b))
        }
        (Value::Object(r), Value::Object(b)) => {
            r.len() == b.len()
                && r.iter()
                    .all(|(key, r)| b.get(key).is_some_and(|b| same_but_numbers(r, b)))
        }
        _ => reference == backend,
    }
}

fn paths<B: Backend>() -> [(&'static str, Decode, Decode); 4] {
    [
        ("value/owned", owned::<B, AsValue>, owned::<SerdeJson, AsValue>),
        ("value/borrowed", borrowed::<B, AsValue>, borrowed::<SerdeJson, AsValue>),
        ("map/owned", owned::<B, AsMap>, owned::<SerdeJson, AsMap>),
        ("map/borrowed", borrowed::<B, AsMap>, borrowed::<SerdeJson, AsMap>),
    ]
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>(corpus: &Corpus) -> Section {
    let mut section = Section::new("value_targets");
    let large = JsonLarge::bytes();
    let documents = corpus
        .expecting(Expect::Accept)
        .map(|case| (case.name.as_str(), case.bytes.as_slice()))
        .chain([(JsonLarge::NAME, large.as_slice())]);
    for (document, input) in documents {
        for (path, backend, reference) in paths::<B>() {
            let name = format!("{document}#{path}");
            let r = reference(input, Arrival::Unique);
            let b = backend(input, Arrival::Unique);
            section.case();
            if agree(&r, &b) {
                continue;
            }
            section.differ(&name, r.describe(), b.describe());
            if faithful(input, &b) {
                continue;
            }
            match (&r, &b) {
                (Outcome::Accept(r), Outcome::Accept(b)) if NUMBERS_EXCUSED && same_but_numbers(r, b) => {
                    section.note(format!(
                        "{name}: number representation differs under arbitrary_precision"
                    ));
                }
                _ => section.fail(&name, "outcome differs from serde_json"),
            }
        }
    }
    section
}
