//! A request body as a server holds it.

use bytes::{Bytes, BytesMut};

/// A body a borrowed decode may borrow from.
///
/// Starts as the `Bytes` the server received. A backend that parses in place
/// asks for [`Body::make_mut`], which reuses the allocation when the `Bytes`
/// is unique and copies it when it is shared — the copy an in-place parser
/// costs a real server, counted where it happens.
#[derive(Debug)]
pub struct Body {
    frozen: Bytes,
    thawed: Option<BytesMut>,
}

impl Body {
    /// Wrap a received body.
    #[must_use]
    pub fn new(bytes: Bytes) -> Self {
        Self {
            frozen: bytes,
            thawed: None,
        }
    }

    /// The bytes, read-only.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        self.thawed.as_deref().unwrap_or(&self.frozen)
    }

    /// The bytes, writable: zero-copy if unique, a copy otherwise.
    pub fn make_mut(&mut self) -> &mut [u8] {
        let thawed = self.thawed.get_or_insert_with(|| {
            let frozen = std::mem::take(&mut self.frozen);
            frozen
                .try_into_mut()
                .unwrap_or_else(|shared| BytesMut::from(&shared[..]))
        });
        &mut thawed[..]
    }
}

/// Make `bytes` writable the way [`Body::make_mut`] does, for owned decodes.
#[must_use]
pub fn thaw(bytes: Bytes) -> BytesMut {
    bytes
        .try_into_mut()
        .unwrap_or_else(|shared| BytesMut::from(&shared[..]))
}

/// The two ways a server receives a body.
///
/// `Shared` is one HTTP frame split off the connection's read buffer, so it
/// shares that allocation and is not unique. `Unique` is several frames
/// already collected into a fresh buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    /// A single frame sharing the connection buffer.
    Shared,
    /// A unique buffer.
    Unique,
}

impl Arrival {
    /// Stable identifier.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Unique => "unique",
        }
    }

    /// Materialize `input` the way it would arrive. The returned guard keeps
    /// the rest of the shared buffer alive, as the connection would.
    #[must_use]
    pub fn materialize(self, input: &[u8]) -> (Bytes, Option<Bytes>) {
        match self {
            Self::Unique => (Bytes::from(input.to_vec()), None),
            Self::Shared => {
                let mut buffer = BytesMut::with_capacity(input.len() + 64);
                buffer.extend_from_slice(input);
                buffer.extend_from_slice(&[b' '; 64]);
                let mut frozen = buffer.freeze();
                let body = frozen.split_to(input.len());
                (body, Some(frozen))
            }
        }
    }
}
