//! Section `structs`: duplicate and unknown members against derived structs
//! and maps.
//!
//! serde_json refuses a duplicate field in a derived struct (422) but lets a
//! map keep the last value, and refuses unknown fields only under
//! `deny_unknown_fields`. A backend that differs changes what a handler sees
//! for the same body. Every disagreement gates.

use std::collections::BTreeMap;

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use serde::{Deserialize, Serialize};

use crate::outcome::{ARRIVALS, AsValue, Decode, Typed, agree, borrowed, owned};
use crate::record::Section;

/// A plain struct.
#[derive(Debug, Serialize, Deserialize)]
pub struct Plain {
    /// The only field.
    pub a: u32,
}

/// [`Plain`] refusing unknown fields.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strict {
    /// The only field.
    pub a: u32,
}

const INPUTS: &[(&str, &str)] = &[
    ("valid", r#"{"a":1}"#),
    ("duplicate", r#"{"a":1,"a":2}"#),
    ("duplicate-wrong-type", r#"{"a":1,"a":"x"}"#),
    ("unknown", r#"{"a":1,"b":2}"#),
    ("unknown-first", r#"{"b":2,"a":1}"#),
    ("unknown-nested", r#"{"a":1,"b":{"c":[1,{"d":null}]}}"#),
    ("missing", "{}"),
    ("as-sequence", "[1]"),
];

fn paths<B: Backend>() -> [(&'static str, Decode, Decode); 8] {
    [
        (
            "plain/owned",
            owned::<B, Typed<Plain>>,
            owned::<SerdeJson, Typed<Plain>>,
        ),
        (
            "plain/borrowed",
            borrowed::<B, Typed<Plain>>,
            borrowed::<SerdeJson, Typed<Plain>>,
        ),
        (
            "strict/owned",
            owned::<B, Typed<Strict>>,
            owned::<SerdeJson, Typed<Strict>>,
        ),
        (
            "strict/borrowed",
            borrowed::<B, Typed<Strict>>,
            borrowed::<SerdeJson, Typed<Strict>>,
        ),
        ("value/owned", owned::<B, AsValue>, owned::<SerdeJson, AsValue>),
        ("value/borrowed", borrowed::<B, AsValue>, borrowed::<SerdeJson, AsValue>),
        (
            "btreemap/owned",
            owned::<B, Typed<BTreeMap<String, u32>>>,
            owned::<SerdeJson, Typed<BTreeMap<String, u32>>>,
        ),
        (
            "btreemap/borrowed",
            borrowed::<B, Typed<BTreeMap<String, u32>>>,
            borrowed::<SerdeJson, Typed<BTreeMap<String, u32>>>,
        ),
    ]
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>() -> Section {
    let mut section = Section::new("structs");
    for (input_name, input) in INPUTS {
        for (path, backend, reference) in paths::<B>() {
            for arrival in ARRIVALS {
                let name = format!("{input_name}#{path}#{}", arrival.name());
                let r = reference(input.as_bytes(), arrival);
                let b = backend(input.as_bytes(), arrival);
                section.case();
                if !agree(&r, &b) {
                    section.differ(&name, r.describe(), b.describe());
                    section.fail(&name, "outcome differs from serde_json");
                }
            }
        }
    }
    section
}
