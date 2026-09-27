//! Section `encoding`: string escaping, non-finite floats, and error
//! positions.
//!
//! An encoder may escape differently from serde_json and still be right; it
//! is wrong when its output is not JSON or does not mean the same thing. A
//! byte difference is a non-gating difference; invalid or semantically
//! different output gates, and so does failing to encode what serde_json
//! encodes (the server would answer 500). Error line/column against
//! serde_json for every `n_` file is recorded, never gated.

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use codecs::body::Arrival;
use serde::Serialize;
use serde_json::{Value, json};

use crate::corpus::{Corpus, Expect};
use crate::exact;
use crate::outcome::{AsValue, Outcome, guarded, owned};
use crate::record::Section;

/// A float in a struct field.
#[derive(Debug, Serialize)]
pub struct Field<T> {
    /// The value.
    pub x: T,
}

/// Compare `B`'s encoding of `value` with serde_json's.
pub fn check<B: Backend, T: Serialize>(section: &mut Section, case: &str, value: &T) {
    section.case();
    let reference = SerdeJson::encode(value);
    let backend = guarded(|| B::encode(value));
    let reference = match reference {
        Ok(bytes) => bytes,
        Err(failure) => {
            if matches!(backend, Ok(Ok(_))) {
                section.differ(case, json!({ "error": failure.to_string() }), json!("encoded"));
            }
            return;
        }
    };
    let backend = match backend {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(failure)) => {
            section.differ(case, text(&reference), json!({ "error": failure.to_string() }));
            // JSON has no NaN or infinity; serde_json silently writes `null`.
            // Refusing is a louder answer, not a silent difference.
            if non_finite(case) {
                section.note(format!(
                    "{case}: refuses a non-finite float ({failure}); serde_json writes null"
                ));
            } else {
                section.fail(case, format!("encode failed where serde_json succeeds: {failure}"));
            }
            return;
        }
        Err(panic) => {
            section.differ(case, text(&reference), json!({ "panic": panic }));
            section.fail(case, format!("encode panicked: {panic}"));
            return;
        }
    };
    if backend == reference {
        return;
    }
    section.differ(case, text(&reference), text(&backend));
    if let Err(error) = serde_json::from_slice::<Value>(&backend) {
        section.fail(case, format!("output is not JSON: {error}"));
        return;
    }
    match exact::semantically_equal(&reference, &backend) {
        Ok(true) => section.note(format!("{case}: bytes differ, meaning is the same")),
        Ok(false) => section.fail(case, "output means something else"),
        Err(error) => section.fail(case, error),
    }
}

/// Whether a case encodes NaN or an infinity.
fn non_finite(case: &str) -> bool {
    ["/nan/", "/inf/", "/-inf/"].iter().any(|k| case.contains(k))
}

/// Bytes as text for the report.
fn text(bytes: &[u8]) -> Value {
    json!(String::from_utf8_lossy(bytes))
}

/// The characters whose escaping is checked.
#[must_use]
pub fn characters() -> Vec<char> {
    let mut chars: Vec<char> = (0u8..0x20).map(char::from).collect();
    chars.extend(['\u{7f}', 'é', '中', '😀', '/', '<', '\u{2028}', '\u{2029}', '"', '\\']);
    chars
}

fn encode_part<B: Backend>(section: &mut Section) {
    let chars = characters();
    for &c in &chars {
        check::<B, _>(section, &format!("string/U+{:04X}", u32::from(c)), &format!("a{c}b"));
    }
    check::<B, _>(section, "string/all", &chars.iter().collect::<String>());
    for (name, x) in [("nan", f64::NAN), ("inf", f64::INFINITY), ("-inf", f64::NEG_INFINITY)] {
        check::<B, _>(section, &format!("f64/{name}/field"), &Field { x });
        check::<B, _>(section, &format!("f64/{name}/bare"), &x);
        check::<B, _>(section, &format!("f32/{name}/field"), &Field { x: x as f32 });
        check::<B, _>(section, &format!("f64/{name}/value"), &Value::from(x));
    }
}

fn line_column<B: Backend>(section: &mut Section, corpus: &Corpus) {
    let (mut same, mut differ, mut missing) = (0usize, 0usize, 0usize);
    let mut mismatches = Vec::new();
    for case in corpus.expecting(Expect::Reject) {
        let r = owned::<SerdeJson, AsValue>(&case.bytes, Arrival::Unique);
        let b = owned::<B, AsValue>(&case.bytes, Arrival::Unique);
        let (Outcome::Reject(r), Outcome::Reject(b)) = (r, b) else {
            continue;
        };
        let r = r.line_column(&case.bytes);
        let b = b.line_column(&case.bytes);
        match b {
            None => missing += 1,
            Some(_) if b == r => same += 1,
            Some(_) => {
                differ += 1;
                if mismatches.len() < crate::record::DIFFERENCE_CAP {
                    mismatches.push(json!({ "case": case.name, "reference": r, "backend": b }));
                }
            }
        }
    }
    section.data(
        "line_column",
        json!({ "same": same, "different": differ, "backend_has_none": missing, "mismatches": mismatches }),
    );
    if differ + missing > 0 {
        section.note(format!(
            "error position: {differ} n_ case(s) differ from serde_json, {missing} have none (data.line_column)"
        ));
    }
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>(corpus: &Corpus) -> Section {
    let mut section = Section::new("encoding");
    if B::ENCODES {
        encode_part::<B>(&mut section);
    } else {
        section.note("decode-only backend: encoding checks skipped");
    }
    line_column::<B>(&mut section, corpus);
    section
}
