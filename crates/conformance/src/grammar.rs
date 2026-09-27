//! Section `grammar`: RFC 8259 acceptance over JSONTestSuite.
//!
//! Every file is decoded into `serde_json::Value` and `IgnoredAny`, owned and
//! borrowed, through both arrivals. A `y_` file must be accepted, and into
//! `Value` must decode to serde_json's value or be faithful to the input text
//! (see [`faithful`]); an `n_` file must be
//! rejected. Both are gating. `i_` outcomes are recorded against serde_json
//! and never gate. A panic always gates.

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use serde_json::{Map, Value, json};

use crate::corpus::{Corpus, Expect};
use crate::outcome::{ARRIVALS, AsValue, Decode, Ignored, Outcome, agree, borrowed, faithful, owned};
use crate::record::Section;

/// The decode paths, as `(name, whether it yields a comparable value,
/// backend, reference)`.
fn paths<B: Backend>() -> [(&'static str, bool, Decode, Decode); 4] {
    [
        ("value/owned", true, owned::<B, AsValue>, owned::<SerdeJson, AsValue>),
        (
            "value/borrowed",
            true,
            borrowed::<B, AsValue>,
            borrowed::<SerdeJson, AsValue>,
        ),
        ("ignored/owned", false, owned::<B, Ignored>, owned::<SerdeJson, Ignored>),
        (
            "ignored/borrowed",
            false,
            borrowed::<B, Ignored>,
            borrowed::<SerdeJson, Ignored>,
        ),
    ]
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>(corpus: &Corpus) -> Section {
    let mut section = Section::new("grammar");
    let mut implementation_defined = Map::new();
    for case in &corpus.cases {
        for (path, yields_value, backend, reference) in paths::<B>() {
            for arrival in ARRIVALS {
                let name = format!("{}#{path}#{}", case.name, arrival.name());
                let r = reference(&case.bytes, arrival);
                let b = backend(&case.bytes, arrival);
                section.case();
                let agrees = agree(&r, &b);
                if !agrees {
                    section.differ(&name, r.describe(), b.describe());
                }
                match (&b, case.expect) {
                    (Outcome::Panic(message), _) => section.fail(&name, format!("panicked: {message}")),
                    (Outcome::Reject(failure), Expect::Accept) => {
                        section.fail(&name, format!("y_ file rejected: {failure}"));
                    }
                    (Outcome::Accept(_), Expect::Accept)
                        if yields_value && r.is_accept() && !agrees && !faithful(&case.bytes, &b) =>
                    {
                        section.fail(&name, "decoded value differs from serde_json");
                    }
                    (Outcome::Accept(_), Expect::Reject) => section.fail(&name, "n_ file accepted"),
                    _ => {}
                }
                if case.expect == Expect::Either && !agrees {
                    implementation_defined.insert(name, json!({ "reference": verdict(&r), "backend": verdict(&b) }));
                }
            }
        }
    }
    section.data(
        "implementation_defined_differences",
        Value::Object(implementation_defined),
    );
    section
}

fn verdict(outcome: &Outcome) -> Value {
    match outcome {
        Outcome::Accept(_) => json!("accept"),
        Outcome::Reject(failure) => json!(format!("reject {}", failure.status())),
        Outcome::Panic(_) => json!("panic"),
    }
}
