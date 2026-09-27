//! Section `value_targets`: decoding into serde_json's own dynamic types.
//!
//! A server often takes `serde_json::Value` or `Map<String, Value>` as a
//! body. Their `Deserialize` impls are serde_json's, driven by the backend's
//! deserializer, so every `y_` file and `json-large` must decode to exactly
//! serde_json's result. Under `arbitrary_precision`, `Value`'s `Deserialize`
//! expects serde_json's private number token, which other backends never
//! send; there a disagreement is recorded as a note, not a gate.

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use codecs::body::Arrival;
use payloads::Workload;
use payloads::kynos::JsonLarge;

use crate::corpus::{Corpus, Expect};
use crate::outcome::{AsMap, AsValue, Decode, agree, borrowed, faithful, owned};
use crate::record::Section;

/// Whether disagreements gate in this build.
pub const GATING: bool = !cfg!(feature = "arbitrary-precision");

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
            if GATING {
                section.fail(&name, "outcome differs from serde_json");
            } else {
                section.note(format!("{name}: differs under arbitrary_precision"));
            }
        }
    }
    section
}
