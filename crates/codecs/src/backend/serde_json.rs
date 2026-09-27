//! The baseline.

use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Backend, Body, Class, Failure, Position};

/// `serde_json::{from_slice, to_vec, to_writer}`.
#[derive(Debug)]
pub struct SerdeJson;

/// Classify a serde_json error. Shared: several backends route through
/// serde_json types.
#[must_use]
pub fn failure(error: &serde_json::Error) -> Failure {
    let class = match error.classify() {
        serde_json::error::Category::Io => Class::Io,
        serde_json::error::Category::Syntax => Class::Syntax,
        serde_json::error::Category::Data => Class::Data,
        serde_json::error::Category::Eof => Class::Eof,
    };
    Failure::new(class, error).at(Position::LineColumn(error.line(), error.column()))
}

impl Backend for SerdeJson {
    const NAME: &'static str = "serde_json";
    const CRATE: &'static str = "serde_json";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = true;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        serde_json::from_slice(&body).map_err(|e| failure(&e))
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        let body: &'a Body = body;
        serde_json::from_slice(body.as_slice()).map_err(|e| failure(&e))
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        serde_json::to_vec(value).map_err(|e| failure(&e))
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.clear();
        serde_json::to_writer(out, value).map_err(|e| failure(&e))
    }
}
