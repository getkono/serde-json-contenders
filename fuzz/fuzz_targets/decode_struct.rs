//! Arbitrary bytes into a struct using the attributes a server's types use,
//! owned and borrowed: same accept/reject, same status, same value.
//!
//! The owned decode takes the body as a unique buffer and the borrowed one
//! as a frame sharing the connection's buffer, so an in-place parser runs
//! both its zero-copy path and the copy a shared body costs it.
#![no_main]

use std::borrow::Cow;
use std::collections::BTreeMap;

use codecs::body::Arrival;
use codecs::{Backend, Body, Failure};
use fuzz::Check;
use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    id: u64,
    #[serde(alias = "label")]
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    score: Option<f64>,
    kind: Kind,
    #[serde(default)]
    extra: BTreeMap<String, i64>,
}

/// [`Request`], borrowing its strings where the input allows.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestRef<'a> {
    id: u64,
    #[serde(alias = "label", borrow)]
    name: Cow<'a, str>,
    #[serde(default, borrow)]
    tags: Vec<Cow<'a, str>>,
    score: Option<f64>,
    kind: Kind,
    #[serde(default, borrow)]
    extra: BTreeMap<Cow<'a, str>, i64>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Kind {
    Plain,
    Sized { width: u32, height: u32 },
}

/// Panic unless the two decodes agree.
fn compare<T: Serialize + std::fmt::Debug>(
    form: &str,
    reference: serde_json::Result<T>,
    candidate: Result<T, Failure>,
) {
    match (reference, candidate) {
        (Ok(r), Ok(c)) => {
            let (r, c) = (serde_json::to_value(&r).ok(), serde_json::to_value(&c).ok());
            assert!(
                matches!((&r, &c), (Some(r), Some(c)) if fuzz::same(r, c)),
                "{form}: value differs: {r:?} vs {c:?}"
            );
        }
        (Err(r), Err(c)) => {
            let r = if r.is_data() { 422 } else { 400 };
            assert_eq!(r, c.status(), "{form}: status differs: {c}");
        }
        (Ok(r), Err(c)) => panic!("{form}: refused what serde_json accepts ({r:?}): {c}"),
        (Err(r), Ok(c)) => panic!("{form}: accepted what serde_json refuses ({r}): {c:?}"),
    }
}

struct DecodeStruct;

impl Check for DecodeStruct {
    fn check<B: Backend>(data: &[u8]) {
        compare(
            "owned",
            serde_json::from_slice::<Request>(data),
            B::decode::<Request>(bytes::Bytes::copy_from_slice(data)),
        );
        let (shared, _connection) = Arrival::Shared.materialize(data);
        let mut body = Body::new(shared);
        compare(
            "borrowed",
            serde_json::from_slice::<RequestRef<'_>>(data),
            B::decode_borrowed::<RequestRef<'_>>(&mut body),
        );
    }
}

fuzz_target!(|data: &[u8]| fuzz::run::<DecodeStruct>(data));
