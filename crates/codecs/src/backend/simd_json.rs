//! simd-json: runtime CPU detection, parses in place.

use std::cell::RefCell;

use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::body::thaw;
use crate::{Backend, Body, Class, Failure, Position};

fn failure(error: &simd_json::Error) -> Failure {
    use simd_json::ErrorType;
    // simd-json's own `is_data()` is "anything not otherwise classified", so a
    // malformed literal (`[fals]`) and its depth limit would read as a shape
    // error. A literal error at a `t`/`f`/`n` byte is the input's syntax; the
    // same variants elsewhere are a type mismatch.
    let literal = matches!(error.character(), Some('t' | 'f' | 'n'));
    let class = if matches!(
        error.error(),
        ErrorType::ExpectedTrue | ErrorType::ExpectedFalse | ErrorType::ExpectedNull
    ) && literal
        || matches!(error.error(), ErrorType::DepthLimitExceeded)
    {
        Class::Syntax
    } else if error.is_io() {
        Class::Io
    } else if error.is_eof() {
        Class::Eof
    } else if error.is_syntax() {
        Class::Syntax
    } else {
        Class::Data
    };
    Failure::new(class, error).at(Position::Offset(error.index()))
}

/// `simd_json::serde::from_slice` over the body made writable (zero-copy when
/// the `Bytes` is unique, one copy when it is shared).
#[derive(Debug)]
pub struct SimdJson;

impl Backend for SimdJson {
    const NAME: &'static str = "simd-json";
    const CRATE: &'static str = "simd-json";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = true;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        let mut bytes = thaw(body);
        simd_json::serde::from_slice(&mut bytes).map_err(|e| failure(&e))
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        simd_json::serde::from_slice(body.make_mut()).map_err(|e| failure(&e))
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        simd_json::serde::to_vec(value).map_err(|e| failure(&e))
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.clear();
        simd_json::serde::to_writer(out, value).map_err(|e| failure(&e))
    }
}

thread_local! {
    /// Parser scratch reused across calls on one thread, as a server worker
    /// would hold it.
    static BUFFERS: RefCell<simd_json::Buffers> = RefCell::new(simd_json::Buffers::default());
}

/// [`SimdJson`] with `from_slice_with_buffers` and a per-thread
/// `simd_json::Buffers`, so repeated decodes reuse their scratch.
#[derive(Debug)]
pub struct SimdJsonBuffers;

impl Backend for SimdJsonBuffers {
    const NAME: &'static str = "simd-json-buffers";
    const CRATE: &'static str = "simd-json";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = true;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        let mut bytes = thaw(body);
        BUFFERS.with_borrow_mut(|buffers| {
            simd_json::serde::from_slice_with_buffers(&mut bytes, buffers).map_err(|e| failure(&e))
        })
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        let input = body.make_mut();
        BUFFERS.with_borrow_mut(|buffers| {
            simd_json::serde::from_slice_with_buffers(input, buffers).map_err(|e| failure(&e))
        })
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        SimdJson::encode(value)
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        SimdJson::encode_into(value, out)
    }
}
