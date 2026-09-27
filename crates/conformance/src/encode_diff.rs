//! Section `encode_diff`: every workload, encoded and decoded, against
//! serde_json.
//!
//! Encode side: `encode` and `encode_into` (into a buffer holding stale
//! bytes, which must be cleared) must produce serde_json's bytes, or bytes
//! [`crate::exact`] finds semantically equal (a non-gating difference, listed
//! with the first differing offset); anything else gates, as does failing to
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
use crate::outcome::{ARRIVALS, BorrowedForm, Outcome, Typed, agree, borrowed, guarded, owned};
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

fn compare_encoding(
    section: &mut Section,
    case: &str,
    reference: &[u8],
    backend: Result<Result<Vec<u8>, codecs::Failure>, String>,
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
    match exact::semantically_equal(reference, &backend) {
        Ok(true) => section.differ(case, json!("semantically equal"), difference),
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
                    );
                    let into = guarded(|| {
                        let mut out = b"stale bytes the encoder must clear".to_vec();
                        B::encode_into(&value, &mut out).map(|()| out)
                    });
                    compare_encoding(section, &format!("{}#encode_into", W::NAME), &reference, into);
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
