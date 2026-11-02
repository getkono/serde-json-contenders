//! `conform`: the differential conformance report for this build.
//!
//! `conform` prints one JSON report on stdout covering every compiled
//! backend but the reference. Each backend's suite runs in its own child
//! (`conform suite <backend>`), so a backend that crashes the process is a
//! finding in the report, not a lost report. A child the harness could not
//! run or read is an `error` on that backend's entry instead, and `conform`
//! exits non-zero. `conform depth-probe ...` is the depth section's child; see
//! `conformance::depth`.

use std::io::{self, Read};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use codecs::{Backend, Visit};
use conformance::corpus::{self, Corpus};
use conformance::{Suite, crashed, error_report, harness_failure, report};
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
        Err(error) => return harness_failure(&format!("no current executable: {error}")),
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
        Err(error) => return harness_failure(&format!("could not spawn the suite: {error}")),
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let status = child.wait();
    eprintln!("conform: {backend}: {:.1}s", start.elapsed().as_secs_f64());
    entry(status, &stdout)
}

/// The backend's entry from how its child ended: only a fault signal is the
/// library's doing.
fn entry(status: io::Result<ExitStatus>, stdout: &str) -> Value {
    match status {
        Ok(status) if status.success() => serde_json::from_str(stdout)
            .unwrap_or_else(|error| harness_failure(&format!("unreadable suite output: {error}"))),
        Ok(status) if library_abort(status) => crashed(&format!("suite process failed: {status}")),
        Ok(status) => harness_failure(&format!("suite process failed: {status}")),
        Err(error) => harness_failure(&format!("wait failed: {error}")),
    }
}

/// Whether the child died of a signal its own code raises: a stack overflow's
/// SIGSEGV or SIGABRT, or another fault. SIGKILL, what the OOM killer sends,
/// and every other signal come from outside the process, and an exit code is
/// the suite refusing to run (no corpus, a backend not compiled in).
#[cfg(unix)]
fn library_abort(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal().is_some_and(|signal| {
        [
            libc::SIGSEGV,
            libc::SIGABRT,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
            libc::SIGTRAP,
            libc::SIGSYS,
        ]
        .contains(&signal)
    })
}

#[cfg(not(unix))]
fn library_abort(_: ExitStatus) -> bool {
    false
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
    let unfinished = backends.values().any(|entry| entry["error"].is_string());
    println!("{}", portable(report(backends)));
    if unfinished {
        return ExitCode::from(2);
    }
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

#[cfg(all(test, unix))]
mod tests {
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use serde_json::{Value, json};

    use super::entry;

    fn is_harness_failure(entry: &Value, detail: &str) {
        assert_eq!(entry["gate"], "error", "{entry}");
        assert_eq!(entry["gating_failures"], json!([]), "{entry}");
        assert!(entry["error"].as_str().is_some_and(|e| e.contains(detail)), "{entry}");
    }

    fn signalled(signal: i32) -> ExitStatus {
        ExitStatus::from_raw(signal)
    }

    fn exited(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    #[test]
    fn a_fault_signal_is_a_library_crash() {
        for signal in [libc::SIGSEGV, libc::SIGABRT, libc::SIGBUS, libc::SIGILL] {
            let entry = entry(Ok(signalled(signal)), "");
            assert_eq!(entry["gate"], "fail", "{entry}");
            assert_eq!(entry["gating_failures"][0]["section"], "process", "{entry}");
            assert!(entry["error"].is_null(), "{entry}");
        }
    }

    #[test]
    fn a_signal_from_outside_is_a_harness_failure() {
        for signal in [libc::SIGKILL, libc::SIGTERM, libc::SIGINT] {
            is_harness_failure(&entry(Ok(signalled(signal)), ""), "suite process failed");
        }
    }

    #[test]
    fn an_exit_code_is_a_harness_failure() {
        for code in [1, 2, 3, 101] {
            is_harness_failure(&entry(Ok(exited(code)), ""), "suite process failed");
        }
    }

    #[test]
    fn a_failed_wait_is_a_harness_failure() {
        is_harness_failure(&entry(Err(io::Error::other("gone")), ""), "wait failed: gone");
    }

    #[test]
    fn unreadable_output_is_a_harness_failure() {
        is_harness_failure(&entry(Ok(exited(0)), "{not json"), "unreadable suite output");
    }

    #[test]
    fn readable_output_is_the_entry() {
        assert_eq!(entry(Ok(exited(0)), r#"{"gate":"pass"}"#), json!({ "gate": "pass" }));
    }
}
