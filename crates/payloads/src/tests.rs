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

struct Facts;

impl Visit for Facts {
    type Output = (usize, String);

    fn visit<W: Workload>(self) -> (usize, String) {
        let bytes = W::bytes();
        // The bytes decode to the value, in both forms, under the baseline.
        // Past serde_json's depth limit there is nothing to decode, and for
        // arbitrary floats serde_json's default parser is not correctly
        // rounded (only `float_roundtrip` is), so those two corpora are held
        // to decoding at all; conformance measures the rounding itself.
        let deep = W::NAME == families::Nested::<10_000>::NAME || W::NAME == families::Nested::<129>::NAME;
        let floats = W::NAME == families::Floats::NAME || W::NAME == world::CanadaLike::NAME;
        if !deep {
            let owned: W::Owned = serde_json::from_slice(&bytes).expect("owned decode");
            let borrowed: W::Borrowed<'_> = serde_json::from_slice(&bytes).expect("borrowed decode");
            if !floats {
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
