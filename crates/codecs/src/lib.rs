//! The contract every JSON backend is held to, and one adapter per backend.
//!
//! The contract is what a server's typed JSON body needs: decode a `Bytes`
//! body into `T` (owned, or borrowing from the body), encode `T` to a fresh
//! `Vec` or into a caller's buffer, and say whether a failure is the client's
//! syntax (400) or a well-formed body of the wrong shape (422).
//!
//! Any copy a backend needs to meet the contract happens inside the adapter,
//! so it is inside every measurement.

pub mod backend;
pub mod body;
pub mod failure;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub use body::Body;
pub use failure::{Class, Failure, Position};

/// A JSON backend, as a server uses one.
pub trait Backend: 'static {
    /// Stable identifier used in result files.
    const NAME: &'static str;
    /// The crate providing it, as named in `Cargo.toml`.
    const CRATE: &'static str;
    /// Whether it can encode at all.
    const ENCODES: bool;
    /// Whether it is a candidate for adoption rather than a reference point.
    const ELIGIBLE: bool;

    /// Decode an owned `T` from a body the server now owns.
    fn decode<T: DeserializeOwned>(body: bytes::Bytes) -> Result<T, Failure>;

    /// Decode a `T` that may borrow from `body`.
    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure>;

    /// Encode into a new buffer (what a server calls today).
    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure>;

    /// Encode into `out`, which is cleared first and may already have capacity.
    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure>;
}

/// Receives a backend type chosen at run time.
pub trait Visit {
    /// What the visit produces.
    type Output;
    /// Called with the backend named on the command line.
    fn visit<B: Backend>(self) -> Self::Output;
}

/// Declares the backends: [`compiled`] lists them and [`dispatch`] resolves
/// one by name. One line per backend, gated on its crate's feature.
macro_rules! registry {
    ($($(#[$gate:meta])* $backend:path),* $(,)?) => {
        /// Every backend compiled into this build, reference first.
        #[must_use]
        // One `push` per gated line is what lets a line compile out.
        #[allow(clippy::vec_init_then_push)]
        pub fn compiled() -> Vec<&'static str> {
            let mut names = Vec::new();
            $( $(#[$gate])* names.push(<$backend as Backend>::NAME); )*
            names
        }

        /// Dispatch `visitor` to the backend called `name`, if this build has it.
        pub fn dispatch<V: Visit>(name: &str, visitor: V) -> Option<V::Output> {
            $( $(#[$gate])* if name == <$backend as Backend>::NAME { return Some(visitor.visit::<$backend>()); } )*
            None
        }
    };
}

registry! {
    backend::serde_json::SerdeJson,
    #[cfg(feature = "sonic-rs")] backend::sonic_rs::SonicRs,
}
