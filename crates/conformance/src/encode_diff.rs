//! Section `encode_diff`: every workload, encoded and decoded, against
//! serde_json.
//!
//! Encode side: `encode` and `encode_into` (into a buffer holding stale
//! bytes, which must be cleared) must produce serde_json's bytes, or bytes
//! [`crate::exact`] finds semantically equal, or bytes serde_json decodes into
//! the workload's type as the same value (an `f32` written widened to `f64`
//! is the same `f32`): a non-gating difference, listed with the first
//! differing offset. Anything else gates, as does failing to
//! encode. Decode side: the canonical bytes decoded owned and borrowed,
//! through both arrivals, must agree with serde_json, except where serde_json
//! refuses on its recursion limit (that policy is `depth`'s to report) or
//! where the backend's value matches the source text exactly and serde_json's
//! does not (serde_json's default float parser is not correctly rounded).

use std::marker::PhantomData;

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use payloads::Workload;
use serde_json::{Value, json};

use crate::exact;
use crate::outcome::{ARRIVALS, BorrowedForm, Outcome, Typed, agree, borrowed, guarded, owned, same_value, to_value};
use crate::record::Section;

/// Bytes either side of the first difference shown in the report.
pub const WINDOW: usize = 40;

/// Where two encodings first differ, with a window of each.
#[must_use]
pub fn first_difference(reference: &[u8], backend: &[u8]) -> Value {
    let offset = reference.iter().zip(backend).take_while(|(a, b)| a == b).count();
    let window = |bytes: &[u8]| {
        let start = offset.saturating_sub(WINDOW / 2).min(bytes.len());
        let end = (start + WINDOW).min(bytes.len());
        String::from_utf8_lossy(&bytes[start..end]).into_owned()
    };
    json!({
        "offset": offset,
        "reference_len": reference.len(),
        "backend_len": backend.len(),
        "reference_window": window(reference),
        "backend_window": window(backend),
    })
}

struct Visit<'s, B> {
    section: &'s mut Section,
    backend: PhantomData<B>,
}

/// Whether serde_json decodes both encodings into `W::Owned` as the same
/// value, so each number is read at its field's type: an `f32` written
/// widened to `f64` is the same `f32`, though not the same `f64`.
fn typed_equal<W: Workload>(reference: &[u8], backend: &[u8]) -> bool {
    let decode = |bytes: &[u8]| serde_json::from_slice::<W::Owned>(bytes).map(|value| to_value(&value));
    matches!((decode(reference), decode(backend)), (Ok(r), Ok(b)) if same_value(&r, &b))
}

/// A typed round-trip comparison, as [`typed_equal`].
type TypedEqual = fn(&[u8], &[u8]) -> bool;

fn compare_encoding(
    section: &mut Section,
    case: &str,
    reference: &[u8],
    backend: Result<Result<Vec<u8>, codecs::Failure>, String>,
    typed: TypedEqual,
) {
    section.case();
    let backend = match backend {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(failure)) => {
            section.differ(case, json!("encoded"), json!({ "error": failure.to_string() }));
            section.fail(case, format!("encode failed: {failure}"));
            return;
        }
        Err(panic) => {
            section.differ(case, json!("encoded"), json!({ "panic": panic }));
            section.fail(case, format!("encode panicked: {panic}"));
            return;
        }
    };
    if backend == reference {
        return;
    }
    let difference = first_difference(reference, &backend);
    // Numbers as JSON values first; failing that, as the workload's types.
    match exact::semantically_equal(reference, &backend) {
        Ok(true) => section.differ(case, json!("semantically equal"), difference),
        Ok(false) if typed(reference, &backend) => section.differ(case, json!("semantically equal"), difference),
        Ok(false) => {
            section.differ(case, json!("semantically different"), difference);
            section.fail(case, "encoding means something else than serde_json's");
        }
        Err(error) => {
            section.differ(case, json!("unparseable"), difference);
            section.fail(case, error);
        }
    }
}

impl<B: Backend> payloads::Visit for Visit<'_, B> {
    type Output = ();

    fn visit<W: Workload>(self) {
        let section = self.section;
        let value = W::value();
        if B::ENCODES {
            match SerdeJson::encode(&value) {
                Ok(reference) => {
                    compare_encoding(
                        section,
                        &format!("{}#encode", W::NAME),
                        &reference,
                        guarded(|| B::encode(&value)),
                        typed_equal::<W>,
                    );
                    let into = guarded(|| {
                        let mut out = b"stale bytes the encoder must clear".to_vec();
                        B::encode_into(&value, &mut out).map(|()| out)
                    });
                    compare_encoding(
                        section,
                        &format!("{}#encode_into", W::NAME),
                        &reference,
                        into,
                        typed_equal::<W>,
                    );
                }
                Err(failure) => section.note(format!("{}: serde_json cannot encode it: {failure}", W::NAME)),
            }
        }
        let bytes = W::bytes();
        for arrival in ARRIVALS {
            let paths = [
                (
                    "owned",
                    owned::<SerdeJson, Typed<W::Owned>>(&bytes, arrival),
                    owned::<B, Typed<W::Owned>>(&bytes, arrival),
                ),
                (
                    "borrowed",
                    borrowed::<SerdeJson, BorrowedForm<W>>(&bytes, arrival),
                    borrowed::<B, BorrowedForm<W>>(&bytes, arrival),
                ),
            ];
            for (path, r, b) in paths {
                let case = format!("{}#decode/{path}#{}", W::NAME, arrival.name());
                section.case();
                if agree(&r, &b) {
                    continue;
                }
                section.differ(&case, r.describe(), b.describe());
                match (&r, &b) {
                    (_, Outcome::Panic(message)) => section.fail(&case, format!("panicked: {message}")),
                    (Outcome::Reject(failure), _) if failure.message.contains("recursion limit") => {
                        section.note(format!("{case}: serde_json refuses at its recursion limit (see depth)"));
                    }
                    (Outcome::Accept(_), Outcome::Accept(value))
                        if exact::parse(&bytes).is_ok_and(|source| exact::matches_value(&source, value)) =>
                    {
                        section.note(format!(
                            "{case}: the backend's value is the source's exactly; serde_json's is not"
                        ));
                    }
                    _ => section.fail(&case, "decode differs from serde_json"),
                }
            }
        }
    }
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>() -> Section {
    let mut section = Section::new("encode_diff");
    if !B::ENCODES {
        section.note("decode-only backend: encode checks skipped, decode checks run");
    }
    for name in payloads::ALL {
        payloads::dispatch(
            name,
            Visit::<B> {
                section: &mut section,
                backend: PhantomData,
            },
        );
    }
    section
}

#[cfg(test)]
mod tests {
    use payloads::families::F32s;

    use super::*;

    /// `values` written as simd-json writes `f32`s: widened to `f64` first.
    fn widened(values: &[f32]) -> Vec<u8> {
        let texts: Vec<String> = values.iter().map(|&f| format!("{:?}", f64::from(f))).collect();
        format!("[{}]", texts.join(",")).into_bytes()
    }

    /// The section record and gating failures for one `f32s` encoding.
    fn judged(output: Vec<u8>) -> (Value, usize) {
        let mut section = Section::new("encode_diff");
        compare_encoding(
            &mut section,
            "f32s#encode",
            &F32s::bytes(),
            Ok(Ok(output)),
            typed_equal::<F32s>,
        );
        let failures = section.failure_count();
        (section.finish().0, failures)
    }

    #[test]
    fn f32s_written_widened_differ_only_in_bytes() {
        let output = widened(&F32s::value());
        assert_eq!(exact::semantically_equal(&F32s::bytes(), &output), Ok(false));
        let (record, failures) = judged(output);
        assert_eq!(failures, 0, "{record}");
        assert_eq!(record["disagreements"], 1);
        assert_eq!(record["differences"][0]["reference"], "semantically equal");
    }

    #[test]
    fn a_different_f32_is_a_semantic_difference() {
        let mut values = F32s::value();
        values[3] = f32::from_bits(values[3].to_bits() + 1);
        let (record, failures) = judged(widened(&values));
        assert_eq!(failures, 1, "{record}");
        assert_eq!(record["differences"][0]["reference"], "semantically different");
    }
}
