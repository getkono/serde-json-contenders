//! flexon, in whichever configuration this build selected.

use bytes::Bytes;
use flexon::serde::de::Kind;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::body::thaw;
use crate::{Backend, Body, Class, Failure};

/// This build's configuration: `rt` (runtime detection, the default) or `ct`
/// (compile-time SIMD paths, default features off).
const CONFIG: &str = if cfg!(feature = "flexon-rt") { "rt" } else { "ct" };

fn failure(error: &flexon::serde::de::Error) -> Failure {
    // `Message` is what serde's `de::Error::custom` builds, which is how a
    // derived `Deserialize` reports a wrong shape.
    let class = match error.kind() {
        Kind::Message(_) => Class::Data,
        Kind::Eof => Class::Eof,
        _ => Class::Syntax,
    };
    Failure::new(class, error)
}

fn encode_failure(error: &flexon::serde::ser::Error) -> Failure {
    Failure::new(Class::Data, format!("{error:?}"))
}

/// `flexon::serde::from_slice` (UTF-8 validated, read-only input).
#[derive(Debug)]
pub struct Flexon;

impl Backend for Flexon {
    const NAME: &'static str = if cfg!(feature = "flexon-rt") {
        "flexon-rt"
    } else {
        "flexon-ct"
    };
    const CRATE: &'static str = "flexon";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = true;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        flexon::serde::from_slice(&body).map_err(|e| failure(&e))
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        let body: &'a Body = body;
        flexon::serde::from_slice(body.as_slice()).map_err(|e| failure(&e))
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        flexon::serde::to_vec(value).map_err(|e| encode_failure(&e))
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.clear();
        flexon::serde::to_writer(out, value).map_err(|e| encode_failure(&e))
    }
}

/// `flexon::serde::from_mut_slice`: parses in place, so escaped strings can
/// borrow too, at the cost of making the body writable.
#[derive(Debug)]
pub struct FlexonMut;

impl Backend for FlexonMut {
    const NAME: &'static str = if cfg!(feature = "flexon-rt") {
        "flexon-rt-mut"
    } else {
        "flexon-ct-mut"
    };
    const CRATE: &'static str = "flexon";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = true;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        let mut bytes = thaw(body);
        flexon::serde::from_mut_slice(&mut bytes).map_err(|e| failure(&e))
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        flexon::serde::from_mut_slice(body.make_mut()).map_err(|e| failure(&e))
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        Flexon::encode(value)
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        Flexon::encode_into(value, out)
    }
}

/// Which configuration this build selected.
#[must_use]
pub const fn config() -> &'static str {
    CONFIG
}
