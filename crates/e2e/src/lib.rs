//! Whole HTTP requests, to size the codec's share of one.
//!
//! `cell` measures a codec in isolation; this crate asks what that is worth
//! once a request also pays for HTTP/1 parsing, body collection, routing,
//! response framing and the executor. The server takes exactly the body path
//! a hyper-based framework such as Kynos takes: the request body is collected
//! with `BodyExt::collect(..).await?.to_bytes()` and that `Bytes` is handed to
//! [`codecs::Backend::decode`]; the response is [`codecs::Backend::encode`]'s
//! `Vec` wrapped in a `Full<Bytes>`.
//!
//! Every backend is paired with the *floor*: the same server, routes, headers
//! and body collection, with the codec removed (pre-encoded responses, echoed
//! bytes, discarded bodies). Whatever the floor costs, no codec can win back,
//! so for any metric `m`, the codec's share of a request is
//! `(m(backend) - m(floor)) / m(backend)`, and the most any swap can gain end
//! to end is bounded by it.
//!
//! Two binaries drive it: `e2e-server` serves over TCP for load generators
//! (throughput and latency), and `e2e-count` runs a fixed number of requests
//! over an in-memory pipe on one thread, deterministic enough for callgrind
//! (instructions per request, no network or scheduler noise).

pub mod loopback;
pub mod service;

/// A benchmarked route: a method, a path and the workload it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    /// `POST /echo`: decode an `Echo`, check it, encode it back.
    EchoPost,
    /// `GET /json/large`: encode a 705-item `Catalog` (~64 KiB).
    JsonLargeGet,
    /// `POST /json/large`: decode a 705-item `Catalog`, answer 204.
    JsonLargePost,
    /// `GET /json/small`: encode one `Item` (~91 bytes).
    JsonSmallGet,
}

impl Route {
    /// Every route, in report order.
    pub const ALL: &'static [Self] = &[
        Self::EchoPost,
        Self::JsonLargeGet,
        Self::JsonLargePost,
        Self::JsonSmallGet,
    ];

    /// Stable identifier used on the command line and in result files.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::EchoPost => "echo-post",
            Self::JsonLargeGet => "json-large-get",
            Self::JsonLargePost => "json-large-post",
            Self::JsonSmallGet => "json-small-get",
        }
    }

    /// The route called `name`, if any.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|route| route.name() == name)
    }

    /// The request method.
    #[must_use]
    pub fn method(self) -> http::Method {
        match self {
            Self::EchoPost | Self::JsonLargePost => http::Method::POST,
            Self::JsonLargeGet | Self::JsonSmallGet => http::Method::GET,
        }
    }

    /// The request path.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::EchoPost => "/echo",
            Self::JsonLargeGet | Self::JsonLargePost => "/json/large",
            Self::JsonSmallGet => "/json/small",
        }
    }

    /// The status a correct server answers a well-formed request with.
    #[must_use]
    pub fn status(self) -> http::StatusCode {
        match self {
            Self::JsonLargePost => http::StatusCode::NO_CONTENT,
            Self::EchoPost | Self::JsonLargeGet | Self::JsonSmallGet => http::StatusCode::OK,
        }
    }

    /// The route serving `method` on `path`; `Err(true)` if the path exists
    /// under another method, `Err(false)` if it does not exist.
    pub fn resolve(method: &http::Method, path: &str) -> Result<Self, bool> {
        let mut known = false;
        for &route in Self::ALL {
            if route.path() == path {
                if route.method() == method {
                    return Ok(route);
                }
                known = true;
            }
        }
        Err(known)
    }
}

/// The backend name of the codec-free server.
pub const FLOOR: &str = "floor";

/// The routes worth measuring for `backend`, or `None` if this build does not
/// have it.
///
/// A decode-only backend (`ENCODES == false`) still serves every route, with
/// serde_json encoding its responses, so only the route that encodes nothing
/// measures it.
#[must_use]
pub fn routes_for(backend: &str) -> Option<&'static [Route]> {
    service::handler(backend).map(|handler| handler.routes())
}
