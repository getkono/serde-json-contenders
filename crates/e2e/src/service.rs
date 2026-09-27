//! The server: routing, body collection, and a handler per backend.
//!
//! Everything a request pays for outside the handler (HTTP/1 parsing, routing,
//! `collect().to_bytes()`, response framing) is shared by every backend and
//! the floor, so differences between them are the codec's.

use std::marker::PhantomData;
use std::sync::Arc;

use bytes::Bytes;
use codecs::backend::serde_json::SerdeJson;
use codecs::{Backend, Failure};
use http::header::{ALLOW, CONTENT_TYPE, HeaderValue};
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use payloads::Workload;
use payloads::kynos::{Catalog, Echo, Item, JsonLarge, JsonSmall, checksum};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{FLOOR, Route};

/// The name of the reference encoder.
const SERDE_JSON: &str = SerdeJson::NAME;

/// Answers one routed request whose body is already collected.
pub trait Handler: Send + Sync + 'static {
    /// The backend name it was built for.
    fn name(&self) -> &'static str;
    /// The backend writing its response bodies; serde_json's bytes are the
    /// reference, anything else is compared semantically.
    fn encoder(&self) -> &'static str;
    /// The routes worth measuring it on.
    fn routes(&self) -> &'static [Route];
    /// The response to `route` carrying `body`.
    fn handle(&self, route: Route, body: Bytes) -> Response<Full<Bytes>>;
}

/// The handler for `backend`: [`Floor`] for [`FLOOR`], a [`Codec`] for any
/// backend this build has, `None` otherwise. Response values are built here,
/// once, never per request.
#[must_use]
pub fn handler(backend: &str) -> Option<Arc<dyn Handler>> {
    if backend == FLOOR {
        return Some(Arc::new(Floor::new()));
    }
    codecs::dispatch(backend, Build)
}

struct Build;

impl codecs::Visit for Build {
    type Output = Arc<dyn Handler>;

    fn visit<B: Backend>(self) -> Self::Output {
        Arc::new(Codec::<B>::new())
    }
}

/// A server whose bodies go through backend `B`.
///
/// Decode-only backends decode through `B` and encode with serde_json.
pub struct Codec<B> {
    large: Catalog,
    small: Item,
    backend: PhantomData<fn() -> B>,
}

impl<B: Backend> Codec<B> {
    /// Build the response values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            large: JsonLarge::value(),
            small: JsonSmall::value(),
            backend: PhantomData,
        }
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        if B::ENCODES {
            B::encode(value)
        } else {
            SerdeJson::encode(value)
        }
    }

    fn respond<T: Serialize>(value: &T) -> Response<Full<Bytes>> {
        match Self::encode(value) {
            Ok(bytes) => json(Bytes::from(bytes)),
            Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
}

impl<B: Backend> Default for Codec<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Backend> std::fmt::Debug for Codec<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Codec")
            .field("backend", &B::NAME)
            .finish_non_exhaustive()
    }
}

impl<B: Backend> Handler for Codec<B> {
    fn name(&self) -> &'static str {
        B::NAME
    }

    fn encoder(&self) -> &'static str {
        if B::ENCODES { B::NAME } else { SERDE_JSON }
    }

    fn routes(&self) -> &'static [Route] {
        if B::ENCODES {
            Route::ALL
        } else {
            &[Route::JsonLargePost]
        }
    }

    /// Decode failures answer [`Failure::status`]; an echo whose checksum
    /// does not match its payload is well-formed but wrong, so 422.
    fn handle(&self, route: Route, body: Bytes) -> Response<Full<Bytes>> {
        match route {
            Route::EchoPost => match B::decode::<Echo>(body) {
                Ok(echo) if checksum(echo.payload.as_bytes()) == echo.checksum => Self::respond(&echo),
                Ok(_) => empty(StatusCode::UNPROCESSABLE_ENTITY),
                Err(failure) => empty(status(&failure)),
            },
            Route::JsonLargeGet => Self::respond(&self.large),
            Route::JsonLargePost => match B::decode::<Catalog>(body) {
                Ok(catalog) if catalog.items.len() as u64 == payloads::kynos::CATALOG_ITEMS => {
                    empty(StatusCode::NO_CONTENT)
                }
                Ok(_) => empty(StatusCode::BAD_REQUEST),
                Err(failure) => empty(status(&failure)),
            },
            Route::JsonSmallGet => Self::respond(&self.small),
        }
    }
}

/// The same server with the codec removed: echoes bodies, serves bytes
/// encoded once at startup by serde_json, and discards what it would decode.
#[derive(Debug)]
pub struct Floor {
    large: Bytes,
    small: Bytes,
}

impl Floor {
    /// Encode the responses once.
    #[must_use]
    pub fn new() -> Self {
        Self {
            large: Bytes::from(JsonLarge::bytes()),
            small: Bytes::from(JsonSmall::bytes()),
        }
    }
}

impl Default for Floor {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler for Floor {
    fn name(&self) -> &'static str {
        FLOOR
    }

    fn encoder(&self) -> &'static str {
        SERDE_JSON
    }

    fn routes(&self) -> &'static [Route] {
        Route::ALL
    }

    fn handle(&self, route: Route, body: Bytes) -> Response<Full<Bytes>> {
        match route {
            Route::EchoPost => json(body),
            Route::JsonLargeGet => json(self.large.clone()),
            Route::JsonLargePost => {
                drop(body);
                empty(StatusCode::NO_CONTENT)
            }
            Route::JsonSmallGet => json(self.small.clone()),
        }
    }
}

fn status(failure: &Failure) -> StatusCode {
    StatusCode::from_u16(failure.status()).unwrap_or(StatusCode::BAD_REQUEST)
}

fn json(body: Bytes) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn empty(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    response
}

/// Route `request`, collect its body as a framework does, and hand it to
/// `handler`. An unknown path is 404; a known path under another method is
/// 405 with `Allow` (RFC 9110 §15.5.6).
pub async fn serve(
    handler: Arc<dyn Handler>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let path = request.uri().path();
    let route = match Route::resolve(request.method(), path) {
        Ok(route) => route,
        Err(false) => return Ok(empty(StatusCode::NOT_FOUND)),
        Err(true) => {
            let allow = Route::ALL
                .iter()
                .filter(|route| route.path() == path)
                .map(|route| route.method().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let mut response = empty(StatusCode::METHOD_NOT_ALLOWED);
            if let Ok(allow) = HeaderValue::from_str(&allow) {
                response.headers_mut().insert(ALLOW, allow);
            }
            return Ok(response);
        }
    };
    let body = request.into_body().collect().await?.to_bytes();
    Ok(handler.handle(route, body))
}

/// Serve HTTP/1 on `io` until the peer closes, configured as Kynos configures
/// hyper: keep-alive on, a 30 s header read timeout on the tokio timer.
pub async fn serve_connection<I>(io: I, handler: Arc<dyn Handler>) -> Result<(), hyper::Error>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .timer(TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(30))
        .serve_connection(
            TokioIo::new(io),
            hyper::service::service_fn(move |request| serve(Arc::clone(&handler), request)),
        )
        .await
}
