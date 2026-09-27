//! sonic-rs: SIMD selected at compile time only.

use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Backend, Body, Class, Failure, Position};

/// `sonic_rs::{from_slice, to_vec, to_writer}`. Never the `_unchecked` entry
/// points, which skip UTF-8 validation.
#[derive(Debug)]
pub struct SonicRs;

fn failure(error: &sonic_rs::Error) -> Failure {
    // sonic-rs splits serde_json's `Data` into a type mismatch and a missing
    // field; both are a well-formed body of the wrong shape.
    let class = match error.classify() {
        sonic_rs::error::Category::Io => Class::Io,
        sonic_rs::error::Category::TypeUnmatched | sonic_rs::error::Category::NotFound => Class::Data,
        sonic_rs::error::Category::Eof => Class::Eof,
        // `Syntax`, and whatever a future version adds to the non-exhaustive
        // enum: nothing but a shape error is a 422.
        _ => Class::Syntax,
    };
    Failure::new(class, error).at(Position::LineColumn(error.line(), error.column()))
}

impl Backend for SonicRs {
    const NAME: &'static str = "sonic-rs";
    const CRATE: &'static str = "sonic-rs";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = true;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        sonic_rs::from_slice(&body).map_err(|e| failure(&e))
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        let body: &'a Body = body;
        sonic_rs::from_slice(body.as_slice()).map_err(|e| failure(&e))
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        sonic_rs::to_vec(value).map_err(|e| failure(&e))
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.clear();
        sonic_rs::to_writer(out, value).map_err(|e| failure(&e))
    }
}
