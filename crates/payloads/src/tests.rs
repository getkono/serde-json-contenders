use std::borrow::Cow;

use sha2::{Digest, Sha256};

use super::*;

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// kynos-bench's golden bodies (`crates/bench-app/golden/*.http` at 5bd67bc),
/// copied as digests because that repository is private. Equality proves the
/// three shapes are the ones a Kynos server is benchmarked on, byte for byte.
#[test]
fn kynos_shapes_match_kynos_bench_golden_bodies() {
    assert_eq!(
        sha(&kynos::JsonSmall::bytes()),
        "aca6359372f90e9b237687e8e64f42c49a0b67469049fc9c33c0e28d38147058"
    );
    assert_eq!(
        sha(&kynos::JsonLarge::bytes()),
        "932d688ff96de76bb674d9012898e43539ea7fab821f88689994fb32f6c4dd7a"
    );
    assert_eq!(
        sha(&kynos::EchoPost::bytes()),
        "d4eaa3b6cc7740ca8814e8192fa200e6ccf9f48ed9d6e630ccc1f6a6b11c5816"
    );
}

/// How far serde_json's default float parser may land from the correctly
/// rounded value on the float corpora: the cell's `ROUNDING_ULPS`, past
/// which a float is a different value. (It lands 2 apart on `floats`.)
const DECODE_ULPS: u64 = 4;

fn to_value<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("a workload value is a JSON value")
}

/// Distance between two finite floats in units in the last place, across
/// zero.
fn ulps(a: f64, b: f64) -> u64 {
    let key = |x: f64| {
        let bits = x.to_bits() as i64;
        if bits < 0 { -(bits & i64::MAX) - 1 } else { bits }
    };
    key(a).abs_diff(key(b))
}

/// The largest float distance between two values of the same shape, or
/// `None` if anything but a float differs.
fn max_ulps(got: &serde_json::Value, expected: &serde_json::Value) -> Option<u64> {
    use serde_json::Value;
    match (got, expected) {
        (Value::Number(g), Value::Number(e)) if g.is_f64() && e.is_f64() => Some(ulps(g.as_f64()?, e.as_f64()?)),
        (Value::Array(g), Value::Array(e)) if g.len() == e.len() => g
            .iter()
            .zip(e)
            .try_fold(0, |max, (g, e)| Some(max_ulps(g, e)?.max(max))),
        (Value::Object(g), Value::Object(e)) if g.len() == e.len() => e
            .iter()
            .try_fold(0, |max, (key, e)| Some(max_ulps(g.get(key)?, e)?.max(max))),
        (g, e) => (g == e).then_some(0),
    }
}

struct Facts;

impl Visit for Facts {
    type Output = (usize, String);

    fn visit<W: Workload>(self) -> (usize, String) {
        let bytes = W::bytes();
        // The bytes decode to the value, in both forms, under the baseline.
        // Past serde_json's depth limit there is nothing to decode, so the
        // deep corpora are checked without it (`nested_bodies_*`). For
        // arbitrary floats serde_json's default parser is not correctly
        // rounded (only `float_roundtrip` is), so the float corpora are held
        // to their value within [`DECODE_ULPS`] here and to it exactly under
        // correct rounding in `float_texts_are_their_values_exactly`;
        // conformance measures each backend's rounding.
        let deep = W::NAME == families::Nested::<10_000>::NAME || W::NAME == families::Nested::<129>::NAME;
        let floats = [families::Floats::NAME, families::F32s::NAME, world::CanadaLike::NAME].contains(&W::NAME);
        if !deep {
            let owned: W::Owned = serde_json::from_slice(&bytes).expect("owned decode");
            let borrowed: W::Borrowed<'_> = serde_json::from_slice(&bytes).expect("borrowed decode");
            if floats {
                let expected = to_value(&W::value());
                for decoded in [to_value(&owned), to_value(&borrowed)] {
                    let ulps = max_ulps(&decoded, &expected);
                    assert!(ulps.is_some_and(|u| u <= DECODE_ULPS), "{}: {ulps:?}", W::NAME);
                }
            } else {
                assert_eq!(encode_canonical(&owned), encode_canonical(&W::value()), "{}", W::NAME);
                assert_eq!(
                    encode_canonical(&borrowed),
                    encode_canonical(&W::value()),
                    "{}",
                    W::NAME
                );
            }
        }
        (bytes.len(), sha(&bytes))
    }
}

/// Every corpus, pinned. A generator change that moves an input fails here
/// and is reviewed as a change to what every result is about.
#[test]
fn corpora_are_pinned() {
    let mut table = String::new();
    for name in ALL {
        let (len, digest) = dispatch(name, Facts).expect("listed workload dispatches");
        table.push_str(&format!("{digest}  {len:>8}  {name}\n"));
    }
    let expected = include_str!("../SHA256SUMS");
    assert_eq!(table, expected, "corpus digests moved:\n{table}");
}

#[test]
fn sweep_bodies_are_exactly_their_size() {
    for (name, size) in [
        (sweep::Sweep::<64>::NAME, 64),
        (sweep::Sweep::<256>::NAME, 256),
        (sweep::Sweep::<1024>::NAME, 1024),
        (sweep::Sweep::<4096>::NAME, 4096),
        (sweep::Sweep::<16384>::NAME, 16384),
        (sweep::Sweep::<65536>::NAME, 65536),
        (sweep::Sweep::<262_144>::NAME, 262_144),
        (sweep::Sweep::<1_048_576>::NAME, 1_048_576),
    ] {
        let (len, _) = dispatch(name, Facts).expect("dispatches");
        assert_eq!(len, size, "{name}");
    }
}

#[test]
fn nested_bodies_have_their_depth() {
    let bytes = families::Nested::<129>::bytes();
    assert_eq!(bytes.iter().filter(|&&b| b == b'{').count(), 129);
    // serde_json's limit is 128, so the baseline itself refuses 129.
    assert!(serde_json::from_slice::<families::Node>(&bytes).is_err());
    assert!(serde_json::from_slice::<families::Node>(&families::Nested::<127>::bytes()).is_ok());
}

/// The `depth` of each level of a `Nested` body, outermost first, read
/// without recursion: every `{` must open a level whose `depth` member is
/// the number of levels already open, the innermost `next` must be `null`,
/// and every level must close.
fn chain_depths(json: &[u8]) -> Vec<u32> {
    let text = std::str::from_utf8(json).expect("UTF-8");
    let mut depths = Vec::new();
    let mut rest = text;
    while let Some(level) = rest.strip_prefix("{\"depth\":") {
        let digits = level.find(|c: char| !c.is_ascii_digit()).expect("a member follows");
        depths.push(level[..digits].parse().expect("a depth"));
        rest = level[digits..].strip_prefix(",\"next\":").expect("a next member");
    }
    let rest = rest.strip_prefix("null").expect("the innermost next is null");
    assert_eq!(rest, "}".repeat(depths.len()), "every level closes, nothing follows");
    depths
}

/// The iterative counterpart of the `Facts` decode for the corpora serde_json
/// refuses: the value serializes to exactly the bytes (on a stack deep enough
/// for serde's recursion), and both the bytes and the value are the chain
/// 0, 1, ..., D - 1.
fn check_nested<const D: u32>() {
    let bytes = families::Nested::<D>::bytes();
    let depths = chain_depths(&bytes);
    assert_eq!(depths, (0..D).collect::<Vec<_>>(), "{}", families::Nested::<D>::NAME);
    let mut level = Some(&families::Nested::<D>::value());
    let mut walked = Vec::new();
    while let Some(node) = level {
        walked.push(node.depth);
        level = node.next.as_deref();
    }
    assert_eq!(walked, depths);
    let serialized = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| encode_canonical(&families::Nested::<D>::value()))
        .expect("spawn")
        .join()
        .expect("serializes");
    assert!(
        serialized == bytes,
        "{} serializes to other bytes",
        families::Nested::<D>::NAME
    );
}

#[test]
fn nested_bodies_decode_to_their_chain() {
    check_nested::<100>();
    check_nested::<127>();
    check_nested::<129>();
    check_nested::<10_000>();
}

/// Every number in `json`, as its text, in document order.
fn number_texts(json: &[u8]) -> Vec<&str> {
    let text = std::str::from_utf8(json).expect("UTF-8");
    let mut numbers = Vec::new();
    let (mut in_string, mut escaped, mut start) = (false, false, None);
    for (i, b) in text.bytes().enumerate() {
        let numeric = !in_string && matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9');
        match (numeric, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                numbers.push(&text[s..i]);
                start = None;
            }
            _ => {}
        }
        if in_string {
            in_string = escaped || b != b'"';
            escaped = !escaped && b == b'\\';
        } else {
            in_string = b == b'"';
        }
    }
    numbers.extend(start.map(|s| &text[s..]));
    numbers
}

/// Under correct rounding (`str::parse`) every float text is its value's
/// bits: the bytes encode the value exactly, whatever a parser makes of them.
#[test]
fn float_texts_are_their_values_exactly() {
    let floats = families::Floats::value();
    let bytes = families::Floats::bytes();
    let texts = number_texts(&bytes);
    assert_eq!(texts.len(), floats.len());
    for (text, value) in texts.iter().zip(&floats) {
        assert_eq!(text.parse::<f64>().map(f64::to_bits), Ok(value.to_bits()), "{text}");
    }

    let f32s = families::F32s::value();
    let bytes = families::F32s::bytes();
    let texts = number_texts(&bytes);
    assert_eq!(texts.len(), f32s.len());
    for (text, value) in texts.iter().zip(&f32s) {
        assert_eq!(text.parse::<f32>().map(f32::to_bits), Ok(value.to_bits()), "{text}");
    }

    let canada = world::CanadaLike::value();
    let coordinates: Vec<f64> = canada.features[0]
        .geometry
        .coordinates
        .iter()
        .flatten()
        .flat_map(|&(lon, lat)| [lon, lat])
        .collect();
    let bytes = world::CanadaLike::bytes();
    let texts = number_texts(&bytes);
    assert_eq!(texts.len(), coordinates.len());
    for (text, value) in texts.iter().zip(&coordinates) {
        assert_eq!(text.parse::<f64>().map(f64::to_bits), Ok(value.to_bits()), "{text}");
    }
}

#[test]
fn f32s_are_finite_and_family_sized() {
    let f32s = families::F32s::value();
    assert!(f32s.iter().all(|f| f.is_finite()));
    assert!(f32s.iter().any(|f| f.is_subnormal()) || f32s.len() < 1000);
    let len = families::F32s::bytes().len();
    assert!(
        (families::FAMILY_BYTES..families::FAMILY_BYTES + 64).contains(&len),
        "{len}"
    );
}

#[test]
fn ints_extremes_hit_every_boundary() {
    let ints = families::IntsExtremes::value();
    assert_eq!(digit_counts(&ints.unsigned), (1..=20).collect());
    assert_eq!(digit_counts(&ints.signed), (1..=19).collect());
    for digits in 1..=19 {
        let high = 10i64.checked_pow(digits).map_or(i64::MAX, |bound| bound - 1);
        assert!(ints.signed.contains(&high) && ints.signed.contains(&-high), "{digits}");
    }
    for exact in [i64::MIN, i64::MAX, 0, -1] {
        assert!(ints.signed.contains(&exact), "{exact}");
    }
    for exact in [u64::MAX, 0, 9_999_999_999_999_999_999, 10_000_000_000_000_000_000] {
        assert!(ints.unsigned.contains(&exact), "{exact}");
    }
}

#[test]
fn long_string_is_one_unescaped_256_kib_string() {
    let value = families::LongString::value();
    assert_eq!(value.text.len(), families::LONG_STRING_BYTES);
    let bytes = families::LongString::bytes();
    assert_eq!(bytes.len(), families::LONG_STRING_BYTES + r#"{"text":""}"#.len());
    assert!(bytes.is_ascii() && !bytes.contains(&b'\\'));
    // Escape-free, so a backend that can borrow does.
    let borrowed: families::TextRef<'_> = serde_json::from_slice(&bytes).expect("decodes");
    assert!(matches!(borrowed.text, Cow::Borrowed(_)));
}

#[test]
fn pretty_corpora_differ_only_in_whitespace() {
    for (pretty, compact) in [
        (families::PrettyJsonLarge::bytes(), kynos::JsonLarge::bytes()),
        (world::PrettyTwitterLike::bytes(), world::TwitterLike::bytes()),
    ] {
        assert!(pretty.len() > compact.len() * 5 / 4);
        assert!(pretty.contains(&b'\n'));
        let a: serde_json::Value = serde_json::from_slice(&pretty).expect("pretty decodes");
        let b: serde_json::Value = serde_json::from_slice(&compact).expect("compact decodes");
        assert_eq!(a, b);
    }
    let pretty = world::PrettyTwitterLike::bytes();
    assert!(pretty.is_ascii());
    assert_eq!(
        pretty.windows(2).filter(|w| w == b"\\u").count(),
        world::TwitterLike::bytes().windows(2).filter(|w| w == b"\\u").count()
    );
}

#[test]
fn raw_utf8_writes_non_ascii_raw() {
    let bytes = world::RawUtf8::bytes();
    assert!(!bytes.is_ascii());
    assert!(!bytes.windows(2).any(|w| w == b"\\u"));
    assert_eq!(ascii_escape(&bytes), world::TwitterLike::bytes());
}

/// Keys in an object, its nested objects and its arrays' objects.
fn keys(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(members) => members.len() + members.values().map(keys).sum::<usize>(),
        serde_json::Value::Array(items) => items.iter().map(keys).sum(),
        _ => 0,
    }
}

#[test]
fn skip_unknown_declares_a_fifth_of_each_status() {
    assert_eq!(world::SkipUnknown::bytes(), world::TwitterLike::bytes());
    let full = to_value(&world::TwitterLike::value());
    let brief = to_value(&world::SkipUnknown::value());
    let (declared, all) = (keys(&brief["statuses"]), keys(&full["statuses"]));
    assert!(
        declared * 100 >= all * 15 && declared * 100 <= all * 25,
        "{declared} of {all}"
    );
    assert_eq!(brief["statuses"].as_array().map(Vec::len), Some(700));
}

#[test]
fn enums_use_every_representation_and_variant() {
    use enums::{Command, Envelope, Event, Payload};
    let records = enums::Enums::value();
    let len = enums::Enums::bytes().len();
    assert!((enums::ENUMS_BYTES..enums::ENUMS_BYTES + 512).contains(&len), "{len}");
    let mut seen = std::collections::BTreeSet::new();
    for record in &records {
        seen.insert(match record.command {
            Command::Stop => "command/stop",
            Command::Resize(_) => "command/resize",
            Command::Move(..) => "command/move",
            Command::Rename { .. } => "command/rename",
        });
        seen.insert(match record.event {
            Event::Created { .. } => "event/created",
            Event::Deleted { .. } => "event/deleted",
            Event::Moved { .. } => "event/moved",
            Event::Heartbeat => "event/heartbeat",
        });
        seen.insert(match record.envelope {
            Envelope::Text(_) => "envelope/text",
            Envelope::Count(_) => "envelope/count",
            Envelope::Pair(..) => "envelope/pair",
            Envelope::Login { .. } => "envelope/login",
            Envelope::Ping => "envelope/ping",
        });
        seen.insert(match record.payload {
            Payload::Nothing => "payload/nothing",
            Payload::Number(_) => "payload/number",
            Payload::Text(_) => "payload/text",
            Payload::List(_) => "payload/list",
            Payload::Error { .. } => "payload/error",
            Payload::Point { .. } => "payload/point",
        });
    }
    assert_eq!(seen.len(), 4 + 4 + 5 + 6, "{seen:?}");
    // Each representation is on the wire as its name says.
    let text = String::from_utf8(enums::Enums::bytes()).expect("UTF-8");
    for marker in [
        r#""command":"stop""#,
        r#""command":{"rename":{"#,
        r#""event":{"type":"#,
        r#""envelope":{"tag":"#,
        r#""content":"#,
        r#""payload":null"#,
    ] {
        assert!(text.contains(marker), "{marker}");
    }
}

/// The digit counts present among `values`, by their decimal text.
fn digit_counts<T: ToString>(values: &[T]) -> std::collections::BTreeSet<usize> {
    values
        .iter()
        .map(|v| v.to_string().trim_start_matches('-').len())
        .collect()
}

#[test]
fn ints_cover_every_digit_count() {
    let ints = families::Ints::value();
    assert_eq!(digit_counts(&ints.unsigned), (1..=20).collect());
    assert_eq!(digit_counts(&ints.signed), (1..=19).collect());
}

#[test]
fn twitter_like_escapes_all_non_ascii() {
    let bytes = world::TwitterLike::bytes();
    assert!(bytes.is_ascii());
    assert!(bytes.windows(2).filter(|w| w == b"\\u").count() > 1000);
}

#[test]
fn escapes_family_is_escape_dense() {
    let bytes = families::Escapes::bytes();
    let escapes = bytes.iter().filter(|&&b| b == b'\\').count();
    assert!(escapes * 10 > bytes.len(), "{escapes} escapes in {}", bytes.len());
}

#[test]
fn dispatch_refuses_unknown_names() {
    assert!(dispatch("no-such-workload", Facts).is_none());
}
