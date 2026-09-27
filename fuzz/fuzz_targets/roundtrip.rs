//! An arbitrary JSON value, encoded by both: each output must be JSON that
//! means the same value, and each backend must read the other's output back.
#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use codecs::Backend;
use fuzz::{Candidate, same};
use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Number, Value};

fn value(u: &mut Unstructured<'_>, depth: u32) -> arbitrary::Result<Value> {
    Ok(match u.int_in_range(0..=if depth > 8 { 5 } else { 7 })? {
        0 => Value::Null,
        1 => Value::Bool(bool::arbitrary(u)?),
        2 => Value::Number(Number::from(i64::arbitrary(u)?)),
        3 => Value::Number(Number::from(u64::arbitrary(u)?)),
        4 => Number::from_f64(f64::arbitrary(u)?).map_or(Value::Null, Value::Number),
        5 => Value::String(String::arbitrary(u)?),
        6 => Value::Array(
            (0..u.int_in_range(0..=6)?)
                .map(|_| value(u, depth + 1))
                .collect::<arbitrary::Result<_>>()?,
        ),
        _ => {
            let mut map = Map::new();
            for _ in 0..u.int_in_range(0..=6)? {
                map.insert(String::arbitrary(u)?, value(u, depth + 1)?);
            }
            Value::Object(map)
        }
    })
}

fuzz_target!(|data: &[u8]| {
    let Ok(original) = value(&mut Unstructured::new(data), 0) else {
        return;
    };
    let reference = serde_json::to_vec(&original).expect("serde_json encodes any Value");
    if let Ok(encoded) = Candidate::encode(&original) {
        let parsed: Value = serde_json::from_slice(&encoded).expect("candidate output is JSON");
        assert!(
            same(&parsed, &original),
            "candidate encoded a different value: {}",
            String::from_utf8_lossy(&encoded)
        );
    }
    let back = Candidate::decode::<Value>(bytes::Bytes::from(reference)).expect("candidate reads serde_json's output");
    assert!(
        same(&back, &original),
        "candidate read serde_json's output as a different value"
    );
});
