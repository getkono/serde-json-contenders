//! `conform`: the differential conformance report for this build.
//!
//! `conform` prints one JSON report on stdout covering every compiled
//! backend but the reference. Each backend's suite runs in its own child
//! (`conform suite <backend>`), so a backend that crashes the process is a
//! finding in the report, not a lost report. `conform depth-probe ...` is the
//! depth section's child; see `conformance::depth`.

use std::io::Read;
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use codecs::{Backend, Visit};
use conformance::corpus::{self, Corpus};
use conformance::{Suite, crashed, error_report, report};
use serde_json::{Map, Value};

/// Backend suites run at once.
const PARALLEL: usize = 2;

struct RunSuite<'a>(&'a Suite);

impl Visit for RunSuite<'_> {
    type Output = Value;
    fn visit<B: Backend>(self) -> Value {
        self.0.run_backend::<B>()
    }
}

fn load() -> Result<Corpus, String> {
    Corpus::load(&corpus::default_dir())
}

/// `conform suite <backend>`: one backend's entry.
fn suite(backend: &str) -> ExitCode {
    conformance::outcome::quiet_guarded_panics();
    let corpus = match load() {
        Ok(corpus) => corpus,
        Err(error) => {
            eprintln!("conform: corpus: {error}");
            return ExitCode::from(2);
        }
    };
    let mut suite = Suite::new(corpus);
    if let Ok(exe) = std::env::current_exe() {
        suite = suite.with_probe(exe);
    }
    let Some(entry) = codecs::dispatch(backend, RunSuite(&suite)) else {
        eprintln!(
            "conform: {backend} is not compiled into this build: {:?}",
            codecs::compiled()
        );
        return ExitCode::from(3);
    };
    println!("{entry}");
    ExitCode::SUCCESS
}

/// One backend's suite in a child process.
fn child_suite(backend: &str) -> Value {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return crashed(&format!("no current executable: {error}")),
    };
    let start = Instant::now();
    let spawned = Command::new(exe)
        .args(["suite", backend])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return crashed(&format!("could not spawn the suite: {error}")),
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let status = child.wait();
    eprintln!("conform: {backend}: {:.1}s", start.elapsed().as_secs_f64());
    match status {
        Ok(status) if status.success() => {
            serde_json::from_str(&stdout).unwrap_or_else(|error| crashed(&format!("unreadable suite output: {error}")))
        }
        Ok(status) => crashed(&format!("suite process failed: {status}")),
        Err(error) => crashed(&format!("wait failed: {error}")),
    }
}

/// The whole report.
fn full() -> ExitCode {
    if let Err(error) = load() {
        println!("{}", error_report(&format!("corpus verification failed: {error}")));
        return ExitCode::from(2);
    }
    let names: Vec<&str> = codecs::compiled().into_iter().skip(1).collect();
    let results = Mutex::new(Map::new());
    let next = Mutex::new(names.iter());
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL {
            scope.spawn(|| {
                // A call, so the lock is released before the suite runs.
                let take = || next.lock().ok().and_then(|mut next| next.next().copied());
                while let Some(name) = take() {
                    let entry = child_suite(name);
                    if let Ok(mut results) = results.lock() {
                        results.insert(name.to_owned(), entry);
                    }
                }
            });
        }
    });
    let backends = results.into_inner().unwrap_or_default();
    println!("{}", portable(report(backends)));
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => full(),
        Some("suite") if args.len() == 2 => suite(&args[1]),
        Some("depth-probe") => ExitCode::from(conformance::depth::child(&args[1..])),
        _ => {
            eprintln!(
                "usage: conform | conform suite <backend> | conform depth-probe <backend> <array|object> <depth> <value|ignored> <stack_bytes>"
            );
            ExitCode::from(2)
        }
    }
}

/// The report with every number a default serde_json can read: under
/// `arbitrary_precision` a `Value` keeps numbers like `1e400` verbatim, which
/// the reader of this report (built without that feature) would refuse.
fn portable(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(n) if n.as_f64().is_none_or(|f| !f.is_finite()) => Value::String(n.to_string()),
        Value::Array(items) => Value::Array(items.into_iter().map(portable).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, portable(v))).collect()),
        other => other,
    }
}
