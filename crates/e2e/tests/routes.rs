//! Every route through the in-process path, and the arrival shapes it relies on.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use e2e::loopback::{self, CHUNK, Loopback, PIPE, body_for, request};
use e2e::service::handler;
use e2e::{FLOOR, Route};
use http::StatusCode;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;

async fn send(backend: &str, route: Route, body: &'static [u8]) -> (StatusCode, Bytes) {
    let mut loopback = Loopback::connect(handler(backend).unwrap()).await.unwrap();
    loopback.send(request(route, &Bytes::from_static(body))).await.unwrap()
}

#[tokio::test]
async fn serde_json_and_floor_answer_every_route_with_serde_json_bytes() {
    for backend in ["serde_json", FLOOR] {
        for &route in Route::ALL {
            loopback::verify(backend, route)
                .await
                .unwrap_or_else(|why| panic!("{backend}: {why}"));
        }
    }
}

#[tokio::test]
async fn malformed_echo_is_400() {
    assert_eq!(
        send("serde_json", Route::EchoPost, b"{\"id\":7,").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn wrong_shape_echo_is_422() {
    let body = br#"{"id":"x","payload":"a","checksum":1}"#;
    assert_eq!(
        send("serde_json", Route::EchoPost, body).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn echo_with_wrong_checksum_is_422() {
    let body = br#"{"id":7,"payload":"a","checksum":1}"#;
    assert_eq!(
        send("serde_json", Route::EchoPost, body).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn catalog_of_wrong_length_is_400() {
    let body = br#"{"revision":1,"total":1,"items":[]}"#;
    assert_eq!(
        send("serde_json", Route::JsonLargePost, body).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn unknown_path_is_404_and_wrong_method_is_405_with_allow() {
    let mut loopback = Loopback::connect(handler("serde_json").unwrap()).await.unwrap();
    let mut missing = request(Route::JsonSmallGet, &Bytes::new());
    *missing.uri_mut() = http::Uri::from_static("/missing");
    assert_eq!(loopback.send(missing).await.unwrap().0, StatusCode::NOT_FOUND);

    let mut client = hyper_client().await;
    let request = http::Request::delete("/json/large")
        .header(http::header::HOST, "localhost")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let response = client.send_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()[http::header::ALLOW], "GET, POST");
}

async fn hyper_client() -> hyper::client::conn::http1::SendRequest<Full<Bytes>> {
    let (client, server) = tokio::io::duplex(PIPE);
    tokio::spawn(e2e::service::serve_connection(server, handler("serde_json").unwrap()));
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client))
        .await
        .unwrap();
    tokio::spawn(connection);
    sender
}

#[test]
fn routes_for_limits_decode_only_backends_and_rejects_unknown_ones() {
    assert_eq!(e2e::routes_for("serde_json"), Some(Route::ALL));
    assert_eq!(e2e::routes_for(FLOOR), Some(Route::ALL));
    assert_eq!(e2e::routes_for("no-such-backend"), None);
    for &route in Route::ALL {
        assert_eq!(Route::parse(route.name()), Some(route));
    }
}

/// How many data frames the server receives for `route`'s canonical body,
/// over the same pipe and client framing the measurements use.
async fn frames(route: Route) -> usize {
    let (client, server) = tokio::io::duplex(PIPE);
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let service = hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
        let seen = Arc::clone(&seen);
        async move {
            let mut body = request.into_body();
            while let Some(frame) = body.frame().await {
                if frame?.is_data() {
                    seen.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok::<_, hyper::Error>(http::Response::new(Full::new(Bytes::new())))
        }
    });
    tokio::spawn(hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(server), service));
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client))
        .await
        .unwrap();
    tokio::spawn(connection);
    sender.send_request(request(route, &body_for(route))).await.unwrap();
    count.load(Ordering::Relaxed)
}

#[tokio::test]
async fn echo_arrives_in_one_frame_and_the_large_body_in_several() {
    assert!(body_for(Route::EchoPost).len() < CHUNK);
    assert_eq!(frames(Route::EchoPost).await, 1);
    assert!(frames(Route::JsonLargePost).await > 1);
}
