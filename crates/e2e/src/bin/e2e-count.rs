//! `e2e-count <backend> <route> [requests]`
//!
//! Runs `requests` (default 1000) sequential requests through one keep-alive
//! connection to an in-process server, on one thread, with no network: the
//! client and server connections talk over a `tokio::io::duplex` pipe on a
//! current-thread runtime (see `e2e::loopback`). The route is first verified
//! against serde_json's bytes and warmed with one request, so lazy statics,
//! CPU-feature detection and buffer growth stay outside the count.
//!
//! Only [`e2e_measured`] is counted: callgrind runs this binary with
//! `--collect-atstart=no --toggle-collect=*e2e_measured*`. Prints one JSON
//! object; exit 1 on a wrong response, 2 on usage, 3 on an unknown backend.

use std::hint::black_box;
use std::process::ExitCode;

use bytes::Bytes;
use e2e::Route;
use e2e::loopback::{Loopback, body_for, verify};
use serde_json::json;
use tokio::runtime::Runtime;

fn usage() -> ExitCode {
    eprintln!("usage: e2e-count <backend|floor> <echo-post|json-large-get|json-large-post|json-small-get> [requests]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [backend, route, rest @ ..] = args.as_slice() else {
        return usage();
    };
    let Some(route) = Route::parse(route) else {
        return usage();
    };
    let requests = match rest.first().map(|n| n.parse::<u64>()) {
        None => 1000,
        Some(Ok(n)) => n,
        Some(Err(_)) => return usage(),
    };
    let Some(handler) = e2e::service::handler(backend) else {
        eprintln!(
            "backend {backend} is not compiled into this build: floor, {:?}",
            codecs::compiled()
        );
        return ExitCode::from(3);
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let body = body_for(route);
    let outcome = runtime.block_on(async {
        verify(backend, route).await?;
        let mut loopback = Loopback::connect(handler).await.map_err(|error| error.to_string())?;
        loopback.exchange(route, &body).await?;
        Ok::<_, String>(loopback)
    });
    let outcome = outcome.and_then(|mut loopback| e2e_measured(&runtime, &mut loopback, route, &body, requests));
    let mut out = json!({ "backend": backend, "route": route.name(), "requests": requests, "ok": outcome.is_ok() });
    if let Err(error) = &outcome {
        out["error"] = json!(error);
    }
    println!("{out}");
    if outcome.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The measured region, synchronous so callgrind's toggle covers every poll
/// of both connections until the last response is collected.
#[inline(never)]
fn e2e_measured(
    runtime: &Runtime,
    loopback: &mut Loopback,
    route: Route,
    body: &Bytes,
    requests: u64,
) -> Result<(), String> {
    runtime.block_on(async {
        for _ in 0..black_box(requests) {
            loopback.exchange(route, body).await?;
        }
        Ok(())
    })
}
