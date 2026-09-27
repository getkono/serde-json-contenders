//! The harness can fail: a backend built to misbehave in known ways is
//! caught exactly where it misbehaves, and serde_json against itself is
//! caught nowhere.

use std::path::PathBuf;

use bytes::Bytes;
use codecs::backend::serde_json::{SerdeJson, failure};
use codecs::{Backend, Body, Class, Failure};
use conformance::Suite;
use conformance::corpus::{Corpus, default_dir};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

/// The one `n_` file the fake accepts, and what it reads it as.
const ACCEPTED_N: (&str, &[u8], &[u8]) = ("n_array_extra_comma.json", br#"["",]"#, br#"[""]"#);

/// serde_json with four deliberate faults: (a) it accepts [`ACCEPTED_N`],
/// (b) it answers a wrong shape with 400, (c) every float it decodes has its
/// lowest mantissa bit flipped, and (d) it encodes `1.5` as `1.50`, which is
/// only a byte difference.
#[derive(Debug)]
struct Fake;

fn misclassify(error: &serde_json::Error) -> Failure {
    let mut failure = failure(error);
    if failure.class == Class::Data {
        failure.class = Class::Syntax;
    }
    failure
}

fn flip_floats(value: &mut Value) {
    match value {
        Value::Number(n) if n.is_f64() => {
            if let Some(flipped) = n
                .as_f64()
                .and_then(|f| Number::from_f64(f64::from_bits(f.to_bits() ^ 1)))
            {
                *n = flipped;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(flip_floats),
        Value::Object(members) => members.values_mut().for_each(flip_floats),
        _ => {}
    }
}

fn decode_slice<'de, T: Deserialize<'de>>(input: &[u8]) -> Result<T, Failure> {
    let input = if input == ACCEPTED_N.1 { ACCEPTED_N.2 } else { input };
    let mut value: Value = serde_json::from_slice(input).map_err(|e| misclassify(&e))?;
    flip_floats(&mut value);
    T::deserialize(value).map_err(|e| misclassify(&e))
}

/// Writes floats with a trailing zero after the shortest fraction.
struct TrailingZero;

impl serde_json::ser::Formatter for TrailingZero {
    fn write_f64<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()> {
        let mut text = format!("{value:?}");
        if text.contains('.') && !text.contains('e') {
            text.push('0');
        }
        writer.write_all(text.as_bytes())
    }
}

impl Backend for Fake {
    const NAME: &'static str = "fake";
    const CRATE: &'static str = "serde_json";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = false;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        decode_slice(&body)
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        decode_slice(body.as_slice())
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        let mut out = Vec::new();
        Self::encode_into(value, &mut out)?;
        Ok(out)
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.clear();
        let mut serializer = serde_json::Serializer::with_formatter(out, TrailingZero);
        value.serialize(&mut serializer).map_err(|e| failure(&e))
    }
}

fn corpus() -> Corpus {
    Corpus::load(&default_dir()).expect("the vendored corpus verifies")
}

fn gating<'a>(entry: &'a Value, section: &str) -> impl Iterator<Item = &'a Value> {
    let section = section.to_owned();
    entry["gating_failures"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |failure| failure["section"] == section.as_str())
}

#[test]
fn fake_faults_are_each_caught_where_they_happen() {
    let entry = Suite::new(corpus()).with_float_samples(10_000).run_backend::<Fake>();
    assert_eq!(entry["gate"], "fail");

    // (a) the accepted n_ file.
    let accepted = format!("{}#value/owned#unique", ACCEPTED_N.0);
    assert!(
        gating(&entry, "grammar").any(|f| f["case"] == accepted.as_str() && f["detail"] == "n_ file accepted"),
        "(a) not caught: {}",
        entry["gating_failures"]
    );

    // (b) a wrong shape answered 400.
    assert!(
        gating(&entry, "status").any(|f| f["case"] == "shape/unknown-field"),
        "(b) not caught: {}",
        entry["sections"]["status"]
    );

    // (c) a float one bit off.
    assert!(
        gating(&entry, "numbers").any(|f| f["case"].as_str().is_some_and(|c| c.starts_with("random/"))),
        "(c) not caught: {}",
        entry["sections"]["numbers"]
    );

    // (d) `1.50`: listed as a difference, never gating.
    let encode_diff = &entry["sections"]["encode_diff"];
    let listed = encode_diff["differences"].as_array().expect("differences");
    assert!(
        listed
            .iter()
            .any(|d| d["case"] == "json-small#encode" && d["reference"] == "semantically equal"),
        "(d) not listed: {encode_diff}"
    );
    assert!(
        !gating(&entry, "encode_diff").any(|f| f["case"].as_str().is_some_and(|c| c.contains("#encode"))),
        "(d) gated: {}",
        entry["gating_failures"]
    );
}

#[test]
fn serde_json_agrees_with_itself_everywhere() {
    let entry = Suite::new(corpus())
        .with_probe(PathBuf::from(env!("CARGO_BIN_EXE_conform")))
        .with_float_samples(100_000)
        .run_backend::<SerdeJson>();
    let sections = entry["sections"].as_object().expect("sections");
    assert_eq!(sections.len(), 10, "{entry}");
    for (name, section) in sections {
        assert!(
            section["cases"].as_u64().unwrap_or(0) > 0,
            "{name} ran no cases: {section}"
        );
        assert_eq!(section["disagreements"], 0, "{name}: {section}");
        assert_eq!(section["gating_failure_count"], 0, "{name}: {section}");
    }
    assert_eq!(entry["gate"], "pass", "{entry}");
}
