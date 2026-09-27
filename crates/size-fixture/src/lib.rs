//! The fixture programs `xtask size` links and weighs.
//!
//! `fixture-full` decodes and re-encodes the two body types a JSON API
//! handler would declare (`Echo`, `Catalog`); `fixture-decode` only decodes
//! them, so a decode-only backend is weighed against the same work;
//! `floor` does the same I/O with no JSON at all. A backend's cost is its
//! fixture's `.text` minus the floor's.

use codecs::Backend;

/// Declares [`Selected`]: the first backend in this list whose feature is on,
/// else serde_json. A measurement build enables one; a test build may enable
/// several, and then the first wins.
macro_rules! select {
    ($($feature:literal => $backend:path),* $(,)?) => {
        select!(@ [] $($feature => $backend),*);
    };
    (@ [$($seen:literal)*] $feature:literal => $backend:path $(, $rf:literal => $rb:path)*) => {
        /// The backend this build weighs.
        #[cfg(all(feature = $feature, not(any($(feature = $seen),*))))]
        pub type Selected = $backend;
        select!(@ [$($seen)* $feature] $($rf => $rb),*);
    };
    (@ [$($seen:literal)*]) => {
        /// The backend this build weighs.
        #[cfg(not(any($(feature = $seen),*)))]
        pub type Selected = codecs::backend::serde_json::SerdeJson;
    };
}

select! {
    "sonic-rs" => codecs::backend::sonic_rs::SonicRs,
    "simd-json" => codecs::backend::simd_json::SimdJson,
    "flexon-rt" => codecs::backend::flexon::Flexon,
    "flexon-ct" => codecs::backend::flexon::Flexon,
    "jiter" => codecs::backend::jiter::Jiter,
    "hifijson" => codecs::backend::hifijson::Hifijson,
    "struson" => codecs::backend::struson::Struson,
}

/// Read all of stdin.
#[must_use]
pub fn stdin() -> bytes::Bytes {
    let mut input = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut input);
    input.into()
}

/// The first byte decides which body type the input is, so the optimizer
/// cannot assume either path away.
#[must_use]
pub fn is_echo(input: &[u8]) -> bool {
    input.get(2) == Some(&b'i')
}

/// Decode (and, when `encode`, re-encode) through [`Selected`].
#[must_use]
pub fn round_trip(input: bytes::Bytes, encode: bool) -> Vec<u8> {
    use payloads::kynos::{Catalog, Echo};
    let out = if is_echo(&input) {
        Selected::decode::<Echo>(input).map(|v| {
            if encode {
                Selected::encode(&v)
            } else {
                Ok(vec![v.payload.len() as u8])
            }
        })
    } else {
        Selected::decode::<Catalog>(input).map(|v| {
            if encode {
                Selected::encode(&v)
            } else {
                Ok(vec![v.items.len() as u8])
            }
        })
    };
    match out {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(failure)) | Err(failure) => failure.status().to_string().into_bytes(),
    }
}
