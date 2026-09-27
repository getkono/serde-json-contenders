//! One module per backend crate. A module compiles only when its crate's
//! feature is on; `serde_json` is always compiled, as the reference.

pub mod serde_json;

#[cfg(any(feature = "flexon-rt", feature = "flexon-ct"))]
pub mod flexon;
#[cfg(feature = "hifijson")]
pub mod hifijson;
#[cfg(feature = "jiter")]
pub mod jiter;
#[cfg(feature = "simd-json")]
pub mod simd_json;
#[cfg(feature = "sonic-rs")]
pub mod sonic_rs;
#[cfg(feature = "struson")]
pub mod struson;
