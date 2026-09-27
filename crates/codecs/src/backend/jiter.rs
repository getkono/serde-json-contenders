//! jiter: decode only.

use bytes::Bytes;
use jiter::JsonErrorType;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Backend, Body, Class, Failure, Position};

fn failure(error: &jiter::serde::Error) -> Failure {
    let class = match error {
        jiter::serde::Error::Syntax(inner) => match inner.error_type {
            JsonErrorType::EofWhileParsingList
            | JsonErrorType::EofWhileParsingObject
            | JsonErrorType::EofWhileParsingString
            | JsonErrorType::EofWhileParsingValue => Class::Eof,
            _ => Class::Syntax,
        },
        jiter::serde::Error::Message(_) | jiter::serde::Error::Data { .. } | jiter::serde::Error::Enum { .. } => {
            Class::Data
        }
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
