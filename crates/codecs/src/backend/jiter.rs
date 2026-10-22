//! jiter: decode only.

use bytes::Bytes;
use jiter::JsonErrorType;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Backend, Body, Class, Failure, Position};

fn failure(error: &jiter::serde::Error) -> Failure {
    use jiter::serde::Error;
    let class = match error {
        Error::Syntax(inner) => match inner.error_type {
            JsonErrorType::EofWhileParsingList
            | JsonErrorType::EofWhileParsingObject
            | JsonErrorType::EofWhileParsingString
            | JsonErrorType::EofWhileParsingValue => Class::Eof,
            // Without `num-bigint` this is only ever a well-formed integer
            // outside `i64`. serde_json reads one as an `f64` and refuses it
            // by type (`2^64` into `u64`), a shape error. Past 308 digits
            // that `f64` overflows and serde_json answers 400 instead, which
            // this error cannot tell apart, so such integers differ the other
            // way. (jiter parses `1e400` to infinity rather than failing.)
            JsonErrorType::NumberOutOfRange => Class::Data,
            _ => Class::Syntax,
        },
        // An enum that is neither a string nor a one-key object (a number,
        // `null`, `{}`, a second key): serde_json refuses every such shape
        // with `ExpectedSomeValue`, a syntax error, before a visitor runs.
        Error::Enum { .. } => Class::Syntax,
        Error::Message(_) | Error::Data { .. } => Class::Data,
    };
    let failure = Failure::new(class, format!("{error:?}"));
    match error.index() {
        Some(index) => failure.at(Position::Offset(index)),
        None => failure,
    }
}

/// `jiter::serde::from_slice`.
#[derive(Debug)]
pub struct Jiter;

impl Backend for Jiter {
    const NAME: &'static str = "jiter";
    const CRATE: &'static str = "jiter";
    const ENCODES: bool = false;
    const ELIGIBLE: bool = true;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        jiter::serde::from_slice(&body).map_err(|e| failure(&e))
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        let body: &'a Body = body;
        jiter::serde::from_slice(body.as_slice()).map_err(|e| failure(&e))
    }

    fn encode<T: Serialize>(_: &T) -> Result<Vec<u8>, Failure> {
        Err(Failure::unsupported())
    }

    fn encode_into<T: Serialize>(_: &T, _: &mut Vec<u8>) -> Result<(), Failure> {
        Err(Failure::unsupported())
    }
}
