//! Arbitrary bytes into `serde_json::Value`: same accept/reject, same value.
#![no_main]

use codecs::Backend;
use fuzz::{Candidate, same};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let reference = serde_json::from_slice::<Value>(data);
    let candidate = Candidate::decode::<Value>(bytes::Bytes::copy_from_slice(data));
    match (reference, candidate) {
        (Ok(r), Ok(c)) => assert!(same(&r, &c), "value differs: {r} vs {c}"),
        (Err(_), Err(c)) => assert_eq!(c.status(), 400, "a non-document is a 400: {c}"),
        (Ok(r), Err(c)) => {
            // Depth limits legitimately differ; any other refusal of a
            // document serde_json accepts is a divergence.
            assert!(fuzz::depth(data) > 128, "refused a valid document ({r}): {c}");
        }
        (Err(r), Ok(c)) => {
            let deep = r.to_string().contains("recursion limit");
            assert!(deep, "accepted what serde_json refuses ({r}): {c}");
        }
    }
});
