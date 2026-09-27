//! Section `numbers`: integer range, float rounding, and float round trips.
//!
//! Truth for a float is `str::parse::<f64>`, which rounds correctly. A
//! backend's float passes if it is bit-identical to the truth or to this
//! build's serde_json (whose default parser is known to be inexact on rare
//! inputs, and exact under `float_roundtrip`); anything else gates. An
//! encoder gates only if its output does not parse back to the exact bits
//! it was given; the other round trips are counted.

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use codecs::body::Arrival;
use payloads::Rng;
use serde_json::{Value, json};

use crate::outcome::{AsValue, Decode, Outcome, Target, Typed, agree, faithful, guarded, owned};
use crate::record::Section;

/// Random floats checked per backend.
pub const SAMPLES: usize = 1_000_000;
/// Their seed.
pub const SEED: u64 = 0x5eed;

/// `f64`, viewed with its bit pattern so equality is exact.
#[derive(Debug)]
pub struct F64;

impl Target for F64 {
    type Out<'de> = f64;
    fn view(out: &f64) -> Value {
        json!({ "f64": format!("{out:?}"), "bits": format!("{:016x}", out.to_bits()) })
    }
}

/// The fixed inputs.
#[must_use]
pub fn fixed() -> Vec<(&'static str, String)> {
    vec![
        ("u64-max", "18446744073709551615".to_owned()),
        ("i64-min", "-9223372036854775808".to_owned()),
        ("2^64", "18446744073709551616".to_owned()),
        ("negative-zero-int", "-0".to_owned()),
        ("negative-zero-float", "-0.0".to_owned()),
        ("1e400", "1e400".to_owned()),
        ("-1e400", "-1e400".to_owned()),
        ("min-subnormal", "5e-324".to_owned()),
        ("min-subnormal-4.9", "4.9e-324".to_owned()),
        ("half-min-subnormal", "2.4703282292062328e-324".to_owned()),
        ("1000-digit-integer", "1234567890".repeat(100)),
        ("0.1", "0.1".to_owned()),
        ("1e23", "1e23".to_owned()),
        ("2^53+1", "9007199254740993".to_owned()),
    ]
}

fn paths<B: Backend>() -> [(&'static str, Decode, Decode); 4] {
    [
        ("u64", owned::<B, Typed<u64>>, owned::<SerdeJson, Typed<u64>>),
        ("i64", owned::<B, Typed<i64>>, owned::<SerdeJson, Typed<i64>>),
        ("f64", owned::<B, F64>, owned::<SerdeJson, F64>),
        ("value", owned::<B, AsValue>, owned::<SerdeJson, AsValue>),
    ]
}

/// Whether an accepted float is the correctly rounded value of `text`.
fn correctly_rounded(path: &str, text: &str, accepted: &Value) -> bool {
    let Ok(truth) = text.parse::<f64>() else { return false };
    let bits = match path {
        "f64" => accepted["bits"].as_str().and_then(|b| u64::from_str_radix(b, 16).ok()),
        "value" => match accepted {
            Value::Number(n) if n.is_f64() || cfg!(feature = "arbitrary-precision") => {
                n.to_string().parse::<f64>().ok().map(f64::to_bits)
            }
            _ => None,
        },
        _ => None,
    };
    bits == Some(truth.to_bits())
}

fn run_fixed<B: Backend>(section: &mut Section) {
    for (name, text) in fixed() {
        for (path, backend, reference) in paths::<B>() {
            let case = format!("{name}#{path}");
            let r = reference(text.as_bytes(), Arrival::Unique);
            let b = backend(text.as_bytes(), Arrival::Unique);
            section.case();
            if agree(&r, &b) {
                continue;
            }
            section.differ(&case, r.describe(), b.describe());
            match (&r, &b) {
                (_, Outcome::Panic(message)) => section.fail(&case, format!("panicked: {message}")),
                (Outcome::Accept(_), Outcome::Accept(accepted))
                    if !(correctly_rounded(path, &text, accepted)
                        || path == "value" && faithful(text.as_bytes(), &b)) =>
                {
                    section.fail(
                        &case,
                        "both accept, values differ, and the backend's is not correctly rounded",
                    );
                }
                _ => {}
            }
        }
    }
}

/// A float decode, as bits.
fn decode_bits<B: Backend>(text: &[u8]) -> Result<u64, String> {
    let body = bytes::Bytes::copy_from_slice(text);
    match guarded(|| B::decode::<f64>(body)) {
        Ok(Ok(value)) => Ok(value.to_bits()),
        Ok(Err(failure)) => Err(failure.to_string()),
        Err(panic) => Err(format!("panicked: {panic}")),
    }
}

/// Counters for the random samples.
#[derive(Debug, Default)]
struct Tally {
    texts: usize,
    backend_inexact: usize,
    reference_inexact: usize,
    backend_rejected: usize,
    encoded: usize,
    encode_failed: usize,
    encoder_inexact: usize,
    backend_to_backend: usize,
    backend_to_reference: usize,
    reference_to_backend: usize,
}

fn hex(bits: &Result<u64, String>) -> Value {
    match bits {
        Ok(bits) => json!(format!("{:?} ({bits:016x})", f64::from_bits(*bits))),
        Err(error) => json!({ "error": error }),
    }
}

fn run_random<B: Backend>(section: &mut Section, samples: usize) -> Tally {
    let mut tally = Tally::default();
    let mut rng = Rng::new(SEED);
    for _ in 0..samples {
        let value = rng.finite_f64();
        let bits = value.to_bits();
        let serde_text = serde_json::to_string(&value).unwrap_or_default();
        let debug_text = format!("{value:?}");
        let texts: &[&str] = if serde_text == debug_text {
            &[&serde_text]
        } else {
            &[&serde_text, &debug_text]
        };
        for (index, text) in texts.iter().enumerate() {
            tally.texts += 1;
            let truth = text.parse::<f64>().map(f64::to_bits).ok();
            let r = decode_bits::<SerdeJson>(text.as_bytes());
            let b = decode_bits::<B>(text.as_bytes());
            section.case();
            tally.reference_inexact += usize::from(r.as_ref().ok() != truth.as_ref());
            tally.backend_inexact += usize::from(b.as_ref().ok() != truth.as_ref());
            tally.backend_rejected += usize::from(b.is_err());
            if index == 0 && b.as_ref().ok() != Some(&bits) {
                tally.reference_to_backend += 1;
            }
            if b != r {
                section.differ(&format!("random/{text}"), hex(&r), hex(&b));
                if b.as_ref().ok() != truth.as_ref() {
                    section.fail(
                        &format!("random/{text}"),
                        format!(
                            "decoded {}, neither the correctly rounded value nor serde_json's",
                            hex(&b)
                        ),
                    );
                }
            }
        }
        if B::ENCODES {
            match guarded(|| B::encode(&value)) {
                Ok(Ok(encoded)) => {
                    tally.encoded += 1;
                    let parsed = std::str::from_utf8(&encoded).ok().and_then(|s| s.parse::<f64>().ok());
                    if parsed.map(f64::to_bits) != Some(bits) {
                        tally.encoder_inexact += 1;
                        section.fail(
                            &format!("encode/{debug_text}"),
                            format!(
                                "encoded as {:?}, which does not parse back to the same bits",
                                String::from_utf8_lossy(&encoded)
                            ),
                        );
                    }
                    tally.backend_to_backend += usize::from(decode_bits::<B>(&encoded) != Ok(bits));
                    tally.backend_to_reference += usize::from(decode_bits::<SerdeJson>(&encoded) != Ok(bits));
                }
                Ok(Err(failure)) => {
                    tally.encode_failed += 1;
                    section.fail(&format!("encode/{debug_text}"), format!("encode failed: {failure}"));
                }
                Err(panic) => {
                    tally.encode_failed += 1;
                    section.fail(&format!("encode/{debug_text}"), format!("encode panicked: {panic}"));
                }
            }
        }
    }
    tally
}

/// Run the section over `samples` random floats.
#[must_use]
pub fn run<B: Backend>(samples: usize) -> Section {
    let mut section = Section::new("numbers");
    run_fixed::<B>(&mut section);
    let tally = run_random::<B>(&mut section, samples);
    section.data(
        "random",
        json!({
            "seed": SEED,
            "samples": samples,
            "texts": tally.texts,
            "backend_not_correctly_rounded": tally.backend_inexact,
            "reference_not_correctly_rounded": tally.reference_inexact,
            "backend_rejected": tally.backend_rejected,
        }),
    );
    if B::ENCODES {
        section.data(
            "round_trip",
            json!({
                "encoded": tally.encoded,
                "encode_failed": tally.encode_failed,
                "encoder_not_round_trip_exact": tally.encoder_inexact,
                "backend_encode_backend_decode_mismatch": tally.backend_to_backend,
                "backend_encode_reference_decode_mismatch": tally.backend_to_reference,
                "reference_encode_backend_decode_mismatch": tally.reference_to_backend,
            }),
        );
    } else {
        section.note("decode-only backend: encoder round trips skipped");
    }
    section
}
