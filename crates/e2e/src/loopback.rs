//! A hyper client wired to the server over an in-memory pipe.
//!
//! No sockets, no second thread: a `tokio::io::duplex` connects a
//! `hyper::client::conn::http1` connection to [`serve_connection`], both
//! spawned on the caller's runtime. On a current-thread runtime the whole
//! exchange is deterministic, which is what instruction counts need. The
//! client's own work (request framing, response parsing) is inside every
//! count, for the floor as for a backend, so it cancels in `backend - floor`
//! but does sit in the denominator of the codec's share.
//!
//! Request bodies go out in [`CHUNK`]-byte frames. `echo-post` (~1.1 KiB)
//! fits one, arrives in the server's first read and is collected as one
//! frame sharing hyper's read buffer; `json-large-post` (~64 KiB) spans
//! several, so `collect().to_bytes()` copies them into a fresh buffer — the
//! two arrivals a real server sees.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderValue};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use hyper::body::{Frame, SizeHint};
use hyper::client::conn::http1::SendRequest;
use hyper_util::rt::TokioIo;
use payloads::Workload;
use payloads::kynos::{EchoPost, JsonLarge, JsonSmall};
use serde_json::Value;

use crate::Route;
use crate::service::{Handler, handler, serve_connection};

/// The pipe's buffer, per direction.
pub const PIPE: usize = 64 * 1024;

/// The largest request body frame the client sends.
pub const CHUNK: usize = 16 * 1024;

/// A request body sent as a fixed sequence of frames.
#[derive(Debug)]
pub struct RequestBody {
    chunks: VecDeque<Bytes>,
    remaining: u64,
}

impl RequestBody {
    /// `bytes`, split into frames of at most [`CHUNK`] bytes. Slices share
    /// `bytes`; nothing is copied.
    #[must_use]
    pub fn chunked(bytes: &Bytes) -> Self {
        let chunks = (0..bytes.len())
            .step_by(CHUNK)
            .map(|start| bytes.slice(start..(start + CHUNK).min(bytes.len())))
            .collect();
        Self {
            chunks,
            remaining: bytes.len() as u64,
        }
    }
}

impl hyper::body::Body for RequestBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let chunk = self.chunks.pop_front();
        if let Some(chunk) = &chunk {
            self.remaining -= chunk.len() as u64;
        }
        Poll::Ready(chunk.map(|chunk| Ok(Frame::data(chunk))))
    }

    fn is_end_stream(&self) -> bool {
        self.chunks.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }
}

/// The canonical request body for `route`; empty for `GET`.
#[must_use]
pub fn body_for(route: Route) -> Bytes {
    match route {
        Route::EchoPost => Bytes::from(EchoPost::bytes()),
        Route::JsonLargePost => Bytes::from(JsonLarge::bytes()),
        Route::JsonLargeGet | Route::JsonSmallGet => Bytes::new(),
    }
}

/// The response body a correct server answers `route` with, as serde_json
/// encodes it; empty for `json-large-post`.
#[must_use]
pub fn expected_for(route: Route) -> Bytes {
    match route {
        Route::EchoPost => Bytes::from(EchoPost::bytes()),
        Route::JsonLargeGet => Bytes::from(JsonLarge::bytes()),
        Route::JsonSmallGet => Bytes::from(JsonSmall::bytes()),
        Route::JsonLargePost => Bytes::new(),
    }
}

/// A request for `route` carrying `body`, with `Host` and, when there is a
/// body, `Content-Type` and `Content-Length`.
#[must_use]
pub fn request(route: Route, body: &Bytes) -> Request<RequestBody> {
    let mut request = Request::new(RequestBody::chunked(body));
    *request.method_mut() = route.method();
    *request.uri_mut() = http::Uri::from_static(route.path());
    let headers = request.headers_mut();
    headers.insert(HOST, HeaderValue::from_static("localhost"));
    if !body.is_empty() {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    }
    request
}

/// One keep-alive client connection to an in-process server.
#[derive(Debug)]
pub struct Loopback {
    sender: SendRequest<RequestBody>,
}

impl Loopback {
    /// Spawn a server connection for `handler` and a client connection to it
    /// on the current runtime.
    pub async fn connect(handler: Arc<dyn Handler>) -> Result<Self, hyper::Error> {
        let (client, server) = tokio::io::duplex(PIPE);
        tokio::spawn(async move {
            // A server error surfaces to the client as a failed exchange.
            let _ = serve_connection(server, handler).await;
        });
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client)).await?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Self { sender })
    }

    /// Send `request` and collect the whole response.
    pub async fn send(&mut self, request: Request<RequestBody>) -> Result<(StatusCode, Bytes), hyper::Error> {
        self.sender.ready().await?;
        let response = self.sender.send_request(request).await?;
        let status = response.status();
        let body = response.into_body().collect().await?.to_bytes();
        Ok((status, body))
    }

    /// Send `route`'s request with `body` and check the status is
    /// [`Route::status`]; the response body is collected and dropped.
    pub async fn exchange(&mut self, route: Route, body: &Bytes) -> Result<(), String> {
        let (status, _) = self
            .send(request(route, body))
            .await
            .map_err(|error| error.to_string())?;
        if status == route.status() {
            Ok(())
        } else {
            Err(format!(
                "{}: status {status}, expected {}",
                route.name(),
                route.status()
            ))
        }
    }
}

/// Serve one canonical request for `route` in-process through `backend` and
/// check the status and body: byte-exact against serde_json's encoding when
/// serde_json wrote the response (serde_json, the floor, decode-only
/// backends), equal as `serde_json::Value` otherwise.
///
/// Must run inside a tokio runtime; the connections are spawned on it.
pub async fn verify(backend: &str, route: Route) -> Result<(), String> {
    let handler = handler(backend).ok_or_else(|| format!("backend {backend} is not compiled into this build"))?;
    let exact = handler.encoder() == <codecs::backend::serde_json::SerdeJson as codecs::Backend>::NAME;
    let mut loopback = Loopback::connect(handler).await.map_err(|error| error.to_string())?;
    let (status, body) = loopback
        .send(request(route, &body_for(route)))
        .await
        .map_err(|error| error.to_string())?;
    if status != route.status() {
        return Err(format!(
            "{}: status {status}, expected {}",
            route.name(),
            route.status()
        ));
    }
    let expected = expected_for(route);
    let equal = if exact || expected.is_empty() {
        body == expected
    } else {
        let parse = |bytes: &[u8]| serde_json::from_slice::<Value>(bytes).map_err(|error| error.to_string());
        parse(&body)? == parse(&expected)?
    };
    if equal {
        Ok(())
    } else {
        Err(format!(
            "{}: body differs from serde_json's ({} bytes, expected {})",
            route.name(),
            body.len(),
            expected.len()
        ))
    }
}
