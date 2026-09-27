//! `e2e-server <backend> <port> [threads]`
//!
//! Serves every route over TCP on `127.0.0.1:<port>` for an external load
//! generator: a multi-thread tokio runtime with `threads` workers (default 4),
//! one hyper HTTP/1 connection per accepted socket, `TCP_NODELAY` on as Kynos
//! sets it. Port 0 picks a free port. Prints `ready <port>` once listening and
//! exits 0 on SIGTERM or ctrl-c. Exit 2 is a usage error, 3 an unknown backend.

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use e2e::service::{Handler, handler, serve_connection};

fn usage() -> ExitCode {
    eprintln!("usage: e2e-server <backend|floor> <port> [threads]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [backend, port, rest @ ..] = args.as_slice() else {
        return usage();
    };
    let Ok(port) = port.parse::<u16>() else {
        return usage();
    };
    let threads = match rest.first().map(|t| t.parse::<usize>()) {
        None => 4,
        Some(Ok(threads)) if threads > 0 => threads,
        Some(_) => return usage(),
    };
    let Some(handler) = handler(backend) else {
        eprintln!(
            "backend {backend} is not compiled into this build: floor, {:?}",
            codecs::compiled()
        );
        return ExitCode::from(3);
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(handler, port)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("e2e-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(handler: Arc<dyn Handler>, port: u16) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "ready {}", listener.local_addr()?.port())?;
    stdout.flush()?;
    drop(stdout);
    let shutdown = shutdown();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok((stream, _)) => stream,
                    // Per-connection failures (such as EMFILE) are transient.
                    Err(error) => {
                        eprintln!("accept: {error}");
                        continue;
                    }
                };
                let _ = stream.set_nodelay(true);
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    let _ = serve_connection(stream, handler).await;
                });
            }
        }
    }
}

/// Resolves on the first SIGTERM or ctrl-c.
async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
