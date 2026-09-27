//! hifijson: decode only, no `unsafe`, a reference point.

use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Backend, Body, Class, Failure};

fn failure(error: &hifijson::serde::Error) -> Failure {
    use hifijson::serde::Error;
    let class = match error {
        Error::Custom(_) | Error::Number(_) => Class::Data,
        Error::Parse(hifijson::Error::Str(hifijson::str::Error::Eof)) => Class::Eof,
        // hifijson does not distinguish running out of input from any other
        // unexpected token outside strings.
        Error::Parse(_) => Class::Syntax,
    };
    Failure::new(class, format!("{error:?}"))
}

fn decode_slice<'a, T: Deserialize<'a>>(input: &'a [u8]) -> Result<T, Failure> {
    let mut lexer = hifijson::SliceLexer::new(input);
    hifijson::serde::exactly_one(&mut lexer).map_err(|e| failure(&e))
}

/// `hifijson::serde::exactly_one` over a `SliceLexer`.
#[derive(Debug)]
pub struct Hifijson;

impl Backend for Hifijson {
    const NAME: &'static str = "hifijson";
    const CRATE: &'static str = "hifijson";
    const ENCODES: bool = false;
    const ELIGIBLE: bool = false;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        decode_slice(&body)
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        let body: &'a Body = body;
        decode_slice(body.as_slice())
    }

    fn encode<T: Serialize>(_: &T) -> Result<Vec<u8>, Failure> {
        Err(Failure::unsupported())
    }

    fn encode_into<T: Serialize>(_: &T, _: &mut Vec<u8>) -> Result<(), Failure> {
        Err(Failure::unsupported())
    }
}
