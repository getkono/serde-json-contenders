//! Section `attributes`: serde's derive attributes, which route through
//! paths a plain struct never touches.
//!
//! `flatten`, `untagged` and internally tagged enums buffer the input into
//! serde's private `Content` before deciding, so they depend on exactly which
//! `visit_*` a backend calls (an integer arriving as `u64` or `f64`, a string
//! as borrowed or owned). Each type gets valid inputs, its variants, and
//! wrong shapes; every disagreement in accept/reject, value
//! (`serde_json::to_value` of the result) or status gates.

use std::borrow::Cow;
use std::collections::BTreeMap;

use codecs::Backend;
use codecs::backend::serde_json::SerdeJson;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::outcome::{ARRIVALS, Decode, Target, Typed, agree, borrowed, owned, to_value};
use crate::record::Section;

/// `flatten` into a map.
#[derive(Debug, Serialize, Deserialize)]
pub struct Flattened {
    /// A named field.
    pub id: u32,
    /// Everything else.
    #[serde(flatten)]
    pub rest: BTreeMap<String, Value>,
}

/// `flatten` of a struct.
#[derive(Debug, Serialize, Deserialize)]
pub struct Outer {
    /// A named field.
    pub name: String,
    /// Flattened fields.
    #[serde(flatten)]
    pub meta: Meta,
}

/// The flattened struct.
#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    /// An integer.
    pub version: u32,
    /// A float.
    pub weight: f64,
}

/// `untagged`: the first variant that fits wins.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Untagged {
    /// A signed integer.
    Int(i64),
    /// A float.
    Float(f64),
    /// A string.
    Text(String),
    /// A pair.
    Pair(u8, u8),
    /// A struct.
    Record {
        /// Its field.
        a: u32,
    },
}

/// Internally tagged.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Internal {
    /// Struct variant with a float.
    Circle {
        /// Radius.
        r: f64,
    },
    /// Struct variant with an integer.
    Square {
        /// Side.
        side: u32,
    },
    /// Unit variant.
    Unit,
}

/// Adjacently tagged.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum Adjacent {
    /// Newtype.
    Num(u32),
    /// Tuple.
    Pair(u8, u8),
    /// Unit.
    Unit,
    /// Struct.
    Record {
        /// Its field.
        a: bool,
    },
}

/// Externally tagged, serde's default.
#[derive(Debug, Serialize, Deserialize)]
pub enum External {
    /// Unit.
    Unit,
    /// Newtype.
    New(u32),
    /// Tuple.
    Tuple(u8, u8),
    /// Struct.
    Record {
        /// Its field.
        a: String,
    },
}

/// `alias`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Aliased {
    /// Also accepted as `old_name` or `legacy`.
    #[serde(alias = "old_name", alias = "legacy")]
    pub name: String,
}

/// `rename_all = "camelCase"`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Camel {
    /// `firstName`.
    pub first_name: String,
    /// `lastSeenAt`.
    pub last_seen_at: u64,
}

fn seven() -> u8 {
    7
}

/// `default` and `skip`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Defaults {
    /// Zero when absent.
    #[serde(default)]
    pub count: u32,
    /// Seven when absent.
    #[serde(default = "seven")]
    pub seven: u8,
    /// Never read.
    #[serde(skip)]
    pub skipped: u32,
    /// Required.
    pub required: bool,
}

/// `Option`, absent vs `null`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Optional {
    /// Absent and `null` are both `None`.
    pub a: Option<u32>,
    /// Nested `Option`: `null` is `None`.
    pub b: Option<Option<u32>>,
}

/// `#[serde(borrow)] Cow<str>`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Borrowing<'a> {
    /// A name.
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    /// Tags.
    #[serde(borrow)]
    pub tags: Vec<Cow<'a, str>>,
}

/// [`Borrowing`] without `borrow`, for the owned decode: `Cow` then never
/// borrows.
#[derive(Debug, Serialize, Deserialize)]
pub struct Owning {
    /// A name.
    pub name: Cow<'static, str>,
    /// Tags.
    pub tags: Vec<Cow<'static, str>>,
}

/// [`Borrowing`] as a decode target.
#[derive(Debug)]
pub struct BorrowingTarget;

impl Target for BorrowingTarget {
    type Out<'de> = Borrowing<'de>;
    fn view(out: &Borrowing<'_>) -> Value {
        to_value(out)
    }
}

/// A type's name, its owned and borrowed decode paths, and its inputs.
type Case = (
    &'static str,
    [(&'static str, Decode, Decode); 2],
    &'static [&'static str],
);

/// Each type's decode paths and inputs.
#[expect(clippy::too_many_lines, reason = "a table of inputs")]
fn cases<B: Backend>() -> Vec<Case> {
    macro_rules! typed {
        ($name:literal, $ty:ty, $inputs:expr) => {
            (
                $name,
                [
                    (
                        "owned",
                        owned::<B, Typed<$ty>> as Decode,
                        owned::<SerdeJson, Typed<$ty>> as Decode,
                    ),
                    (
                        "borrowed",
                        borrowed::<B, Typed<$ty>>,
                        borrowed::<SerdeJson, Typed<$ty>>,
                    ),
                ],
                $inputs,
            )
        };
    }
    vec![
        typed!(
            "flatten-map",
            Flattened,
            &[
                r#"{"id":1}"#,
                r#"{"id":1,"x":1.5,"y":[1,"a"],"z":{"q":null}}"#,
                r#"{"x":-1,"id":2,"big":18446744073709551615}"#,
                r#"{"x":1}"#,
                r#"{"id":"1"}"#,
                "[1]",
            ]
        ),
        typed!(
            "flatten-struct",
            Outer,
            &[
                r#"{"name":"n","version":1,"weight":0.5}"#,
                r#"{"version":1,"weight":2,"name":"n"}"#,
                r#"{"name":"n","version":1}"#,
                r#"{"name":"n","version":-1,"weight":0.5}"#,
                r#"{"name":"n","version":1,"weight":0.5,"extra":true}"#,
            ]
        ),
        typed!(
            "untagged",
            Untagged,
            &[
                "1",
                "-1",
                "1.5",
                "1.0",
                "18446744073709551615",
                "1e2",
                r#""x""#,
                "[1,2]",
                "[1,256]",
                r#"{"a":1}"#,
                "true",
                "null",
            ]
        ),
        typed!(
            "internally-tagged",
            Internal,
            &[
                r#"{"type":"Circle","r":1.5}"#,
                r#"{"r":1,"type":"Circle"}"#,
                r#"{"type":"Square","side":3}"#,
                r#"{"type":"Unit"}"#,
                r#"{"type":"Hexagon"}"#,
                r#"{"r":1.5}"#,
                r#"{"type":"Square","side":-1}"#,
                r#"["Circle",1.5]"#,
            ]
        ),
        typed!(
            "adjacently-tagged",
            Adjacent,
            &[
                r#"{"t":"Num","c":1}"#,
                r#"{"c":[1,2],"t":"Pair"}"#,
                r#"{"t":"Unit"}"#,
                r#"{"t":"Record","c":{"a":true}}"#,
                r#"{"t":"Num"}"#,
                r#"{"t":"Nope","c":1}"#,
                r#"{"t":"Num","c":"1"}"#,
            ]
        ),
        typed!(
            "externally-tagged",
            External,
            &[
                r#""Unit""#,
                r#"{"New":1}"#,
                r#"{"Tuple":[1,2]}"#,
                r#"{"Record":{"a":"x"}}"#,
                r#"{"Unit":null}"#,
                r#""Nope""#,
                r#"{"New":1,"Unit":null}"#,
                r#"{"Tuple":[1]}"#,
            ]
        ),
        typed!(
            "alias",
            Aliased,
            &[
                r#"{"name":"a"}"#,
                r#"{"old_name":"a"}"#,
                r#"{"legacy":"a"}"#,
                r#"{"name":"a","legacy":"b"}"#,
                r#"{"other":"a"}"#,
            ]
        ),
        typed!(
            "rename-all",
            Camel,
            &[
                r#"{"firstName":"a","lastSeenAt":1}"#,
                r#"{"first_name":"a","last_seen_at":1}"#,
                r#"{"firstName":"a"}"#,
                r#"{"firstName":1,"lastSeenAt":1}"#,
            ]
        ),
        typed!(
            "default-skip",
            Defaults,
            &[
                r#"{"required":true}"#,
                r#"{"count":3,"seven":1,"required":false}"#,
                r#"{"skipped":5,"required":true}"#,
                r#"{"count":null,"required":true}"#,
                "{}",
            ]
        ),
        typed!(
            "option",
            Optional,
            &["{}", r#"{"a":null,"b":null}"#, r#"{"a":1,"b":2}"#, r#"{"a":"1"}"#,]
        ),
        (
            "borrow-cow",
            [
                (
                    "owned",
                    owned::<B, Typed<Owning>> as Decode,
                    owned::<SerdeJson, Typed<Owning>> as Decode,
                ),
                (
                    "borrowed",
                    borrowed::<B, BorrowingTarget>,
                    borrowed::<SerdeJson, BorrowingTarget>,
                ),
            ],
            &[
                r#"{"name":"plain","tags":["a","b"]}"#,
                r#"{"name":"esc\"aped","tags":["\u00e9","\n"]}"#,
                r#"{"name":1,"tags":[]}"#,
                r#"{"name":"x"}"#,
            ],
        ),
    ]
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>() -> Section {
    let mut section = Section::new("attributes");
    for (type_name, paths, inputs) in cases::<B>() {
        for (index, input) in inputs.iter().enumerate() {
            for (path, backend, reference) in paths {
                for arrival in ARRIVALS {
                    let name = format!("{type_name}/{index}#{path}#{}", arrival.name());
                    let r = reference(input.as_bytes(), arrival);
                    let b = backend(input.as_bytes(), arrival);
                    section.case();
                    if !agree(&r, &b) {
                        section.differ(
                            &name,
                            serde_json::json!({ "input": input, "outcome": r.describe() }),
                            b.describe(),
                        );
                        section.fail(&name, "outcome differs from serde_json");
                    }
                }
            }
        }
    }
    section
}
