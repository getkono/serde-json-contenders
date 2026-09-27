//! struson: a streaming reader and writer, a reference point.

use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use struson::reader::{JsonReader, JsonStreamReader, ReaderError};
use struson::serde::{DeserializerError, JsonReaderDeserializer, JsonWriterSerializer};
use struson::writer::{JsonStreamWriter, JsonWriter};

use crate::{Backend, Body, Class, Failure, Position};

fn reader_failure(error: &ReaderError) -> Failure {
    let (class, location) = match error {
        ReaderError::SyntaxError(e) => (Class::Syntax, Some(&e.location)),
        ReaderError::UnexpectedValueType { location, .. } | ReaderError::UnexpectedStructure { location, .. } => {
            (Class::Data, Some(location))
        }
        ReaderError::MaxNestingDepthExceeded { location, .. } => (Class::Syntax, Some(location)),
        ReaderError::UnsupportedNumberValue { location, .. } => (Class::Data, Some(location)),
        ReaderError::IoError { error, location } => {
            let class = if error.kind() == std::io::ErrorKind::UnexpectedEof {
                Class::Eof
            } else {
                Class::Io
            };
            (class, Some(location))
        }
        _ => (Class::Syntax, None),
    };
    let failure = Failure::new(class, error);
    match location.and_then(|l| l.line_pos.as_ref()) {
        Some(pos) => failure.at(Position::LineColumn(pos.line as usize + 1, pos.column as usize + 1)),
        None => failure,
    }
}

fn failure(error: &DeserializerError) -> Failure {
    match error {
        DeserializerError::ReaderError(e) => reader_failure(e),
        DeserializerError::MaxNestingDepthExceeded(_) => Failure::new(Class::Syntax, error),
        // `Custom` (a derived `Deserialize` refusing the shape), `InvalidNumber`,
        // and future variants of the non-exhaustive enum.
        _ => Failure::new(Class::Data, error),
    }
}

fn decode_slice<'a, T: Deserialize<'a>>(input: &[u8]) -> Result<T, Failure> {
    let mut reader = JsonStreamReader::new(input);
    let value = {
        let mut deserializer = JsonReaderDeserializer::new(&mut reader);
        T::deserialize(&mut deserializer).map_err(|e| failure(&e))?
    };
    reader.consume_trailing_whitespace().map_err(|e| reader_failure(&e))?;
    Ok(value)
}

/// `JsonReaderDeserializer` / `JsonWriterSerializer` over a slice and a `Vec`.
#[derive(Debug)]
pub struct Struson;

impl Backend for Struson {
    const NAME: &'static str = "struson";
    const CRATE: &'static str = "struson";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = false;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        decode_slice(&body)
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        decode_slice(body.as_slice())
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        let mut out = Vec::new();
        Self::encode_into(value, &mut out)?;
        Ok(out)
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.clear();
        let mut writer = JsonStreamWriter::new(&mut *out);
        let mut serializer = JsonWriterSerializer::new(&mut writer);
        value
            .serialize(&mut serializer)
            .map_err(|e| Failure::new(Class::Data, e))?;
        writer
            .finish_document()
            .map(drop)
            .map_err(|e| Failure::new(Class::Io, e))
    }
}
