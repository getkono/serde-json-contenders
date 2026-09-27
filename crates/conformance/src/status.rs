//! Section `status`: the backend answers the status serde_json answers.
//!
//! A server maps a decode failure to 400 (the body is not JSON) or 422 (JSON
//! of the wrong shape). Three corpora: every `n_` file into `Value`; a set of
//! well-formed bodies of the wrong shape against a derived struct; and every
//! proper prefix of `json-small`, `echo-post` and `json-large` (truncations,
//! which serde_json calls `Eof`). A status mismatch gates; a class mismatch
//! under the same status (`Syntax` vs `Eof`) is a note. An `n_` file the
//! backend accepts is gated by `grammar`, not here.

use std::collections::BTreeMap;

use codecs::backend::serde_json::SerdeJson;
use codecs::body::Arrival;
use codecs::{Backend, Class};
use payloads::Workload;
use payloads::kynos::{EchoPost, JsonLarge, JsonSmall};
use serde::{Deserialize, Serialize};

use crate::corpus::{Corpus, Expect};
use crate::outcome::{AsValue, Outcome, Typed, owned};
use crate::record::Section;

/// A unit enum, for unknown variants.
#[derive(Debug, Serialize, Deserialize)]
pub enum Kind {
    /// First.
    Alpha,
    /// Second.
    Beta,
}

/// The shape the wrong-shape corpus is decoded into.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// A plain integer.
    pub id: u32,
    /// A string.
    pub name: String,
    /// A bool.
    pub flag: bool,
    /// Range-checked unsigned.
    pub small: u8,
    /// Range-checked signed.
    pub signed: i8,
    /// Full-width unsigned.
    pub big: u64,
    /// A float.
    pub score: f64,
    /// A tuple.
    pub pair: (u8, u8),
    /// A fixed array.
    pub triple: [u8; 3],
    /// A unit enum.
    pub kind: Kind,
    /// Optional.
    pub note: Option<String>,
}

const BASE: &[(&str, &str)] = &[
    ("id", "1"),
    ("name", r#""a""#),
    ("flag", "true"),
    ("small", "1"),
    ("signed", "-1"),
    ("big", "1"),
    ("score", "1.5"),
    ("pair", "[1,2]"),
    ("triple", "[1,2,3]"),
    ("kind", r#""Alpha""#),
    ("note", r#""n""#),
];

/// The base document with `field` set to `value` (removed if `None`).
fn with(field: &str, value: Option<&str>) -> String {
    let members: Vec<String> = BASE
        .iter()
        .filter_map(|&(key, base)| {
            let value = if key == field { value? } else { base };
            Some(format!("\"{key}\":{value}"))
        })
        .collect();
    format!("{{{}}}", members.join(","))
}

/// The base document with `extra` members appended.
fn plus(extra: &str) -> String {
    let base = with("", None);
    format!("{},{extra}}}", &base[..base.len() - 1])
}

/// Well-formed bodies of the wrong (and, for control, the right) shape.
#[must_use]
pub fn shape_corpus() -> Vec<(&'static str, String)> {
    vec![
        ("valid", with("", None)),
        ("valid/null-into-option", with("note", Some("null"))),
        ("valid/option-absent", with("note", None)),
        ("wrong-type/string-into-u32", with("id", Some(r#""1""#))),
        ("wrong-type/bool-into-string", with("name", Some("true"))),
        ("wrong-type/object-into-tuple", with("pair", Some(r#"{"a":1}"#))),
        ("wrong-type/array-into-struct", "[1,2]".to_owned()),
        ("wrong-type/string-into-struct", r#""x""#.to_owned()),
        ("wrong-type/null-into-struct", "null".to_owned()),
        ("wrong-type/string-into-f64", with("score", Some(r#""1.5""#))),
        ("missing-field", with("name", None)),
        ("unknown-field", plus(r#""zzz":1"#)),
        ("duplicate-field", plus(r#""id":2"#)),
        ("unknown-variant", with("kind", Some(r#""Omega""#))),
        ("variant-wrong-type", with("kind", Some("1"))),
        ("u8-out-of-range", with("small", Some("256"))),
        ("i8-out-of-range/high", with("signed", Some("128"))),
        ("i8-out-of-range/low", with("signed", Some("-129"))),
        ("u64-out-of-range", with("big", Some("18446744073709551616"))),
        ("negative-into-u64", with("big", Some("-1"))),
        ("negative-zero-into-u64", with("big", Some("-0"))),
        ("float-into-integer", with("id", Some("1.5"))),
        ("integral-float-into-integer", with("id", Some("1.0"))),
        ("exponent-into-integer", with("id", Some("1e2"))),
        ("string-into-bool", with("flag", Some(r#""true""#))),
        ("integer-into-bool", with("flag", Some("1"))),
        ("null-into-non-option", with("name", Some("null"))),
        ("tuple-too-short", with("pair", Some("[1]"))),
        ("tuple-too-long", with("pair", Some("[1,2,3]"))),
        ("array-too-short", with("triple", Some("[1,2]"))),
        ("array-too-long", with("triple", Some("[1,2,3,4]"))),
        ("array-element-wrong-type", with("triple", Some(r#"[1,"2",3]"#))),
    ]
}

/// Tallies class mismatches that keep the status, for the notes.
#[derive(Default)]
struct Classes(BTreeMap<(String, Class, Class), (usize, String)>);

impl Classes {
    fn record(&mut self, corpus: &str, case: &str, reference: &Outcome, backend: &Outcome) {
        if let (Outcome::Reject(r), Outcome::Reject(b)) = (reference, backend)
            && r.status() == b.status()
            && r.class != b.class
        {
            let entry = self
                .0
                .entry((corpus.to_owned(), r.class, b.class))
                .or_insert_with(|| (0, case.to_owned()));
            entry.0 += 1;
        }
    }

    fn notes(self, section: &mut Section) {
        for ((corpus, r, b), (count, first)) in self.0 {
            section.note(format!(
                "{corpus}: {count} case(s) where serde_json says {r:?} and the backend {b:?}, same status (first: {first})"
            ));
        }
    }
}

fn compare(
    section: &mut Section,
    classes: &mut Classes,
    corpus: &str,
    case: &str,
    r: &Outcome,
    b: &Outcome,
    gate_accept: bool,
) {
    section.case();
    classes.record(corpus, case, r, b);
    if r.status() == b.status() {
        return;
    }
    section.differ(case, r.describe(), b.describe());
    match b {
        Outcome::Panic(message) => section.fail(case, format!("panicked: {message}")),
        Outcome::Accept(_) if !gate_accept => {}
        _ => section.fail(case, format!("status {:?}, serde_json {:?}", b.status(), r.status())),
    }
}

/// Every proper prefix of `W`'s bytes.
fn prefixes<B: Backend, W: Workload>(section: &mut Section, classes: &mut Classes) {
    let bytes = W::bytes();
    for len in 0..bytes.len() {
        let input = &bytes[..len];
        let case = format!("prefix/{}/{len}", W::NAME);
        let r = owned::<SerdeJson, Typed<W::Owned>>(input, Arrival::Unique);
        let b = owned::<B, Typed<W::Owned>>(input, Arrival::Unique);
        compare(section, classes, "prefixes", &case, &r, &b, true);
    }
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>(corpus: &Corpus) -> Section {
    let mut section = Section::new("status");
    let mut classes = Classes::default();
    for case in corpus.expecting(Expect::Reject) {
        let r = owned::<SerdeJson, AsValue>(&case.bytes, Arrival::Unique);
        let b = owned::<B, AsValue>(&case.bytes, Arrival::Unique);
        compare(&mut section, &mut classes, "n_", &case.name, &r, &b, false);
    }
    for (name, input) in shape_corpus() {
        let case = format!("shape/{name}");
        let r = owned::<SerdeJson, Typed<Shape>>(input.as_bytes(), Arrival::Unique);
        let b = owned::<B, Typed<Shape>>(input.as_bytes(), Arrival::Unique);
        compare(&mut section, &mut classes, "shape", &case, &r, &b, true);
    }
    prefixes::<B, JsonSmall>(&mut section, &mut classes);
    prefixes::<B, EchoPost>(&mut section, &mut classes);
    prefixes::<B, JsonLarge>(&mut section, &mut classes);
    classes.notes(&mut section);
    section
}
