//! Arbitrary bytes into a struct using the attributes a server's types use:
//! same accept/reject, same status, same value.
#![no_main]

use std::collections::BTreeMap;

use codecs::Backend;
use fuzz::Candidate;
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

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Kind {
    Plain,
    Sized { width: u32, height: u32 },
}

fuzz_target!(|data: &[u8]| {
    let reference = serde_json::from_slice::<Request>(data);
    let candidate = Candidate::decode::<Request>(bytes::Bytes::copy_from_slice(data));
    match (reference, candidate) {
        (Ok(r), Ok(c)) => {
            let (r, c) = (serde_json::to_value(&r).ok(), serde_json::to_value(&c).ok());
            assert!(
                matches!((&r, &c), (Some(r), Some(c)) if fuzz::same(r, c)),
                "value differs: {r:?} vs {c:?}"
            );
        }
        (Err(r), Err(c)) => {
            let r = if r.is_data() { 422 } else { 400 };
            assert_eq!(r, c.status(), "status differs: {c}");
        }
        (Ok(r), Err(c)) => panic!("refused what serde_json accepts ({r:?}): {c}"),
        (Err(r), Ok(c)) => panic!("accepted what serde_json refuses ({r}): {c:?}"),
    }
});
