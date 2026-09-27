//! Section `depth`: how deep a backend nests before refusing, and whether it
//! crashes first.
//!
//! A stack overflow aborts the process, so every probe runs in a child: the
//! `conform` binary re-executed as `conform depth-probe <backend>
//! <array|object> <depth> <value|ignored> <stack_bytes>`, decoding on a
//! thread with that stack. The child exits 0 for accepted, 10 for rejected
//! (printing the status), anything else — or death by a signal — is a crash.
//! The limit itself is recorded, never gated: serde_json's 128 is a choice,
//! not a standard. A crash gates, and so does a rejection answered with 422,
//! since nesting too deep is not a wrong shape.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use codecs::backend::serde_json::SerdeJson;
use codecs::{Backend, Visit};
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use crate::record::Section;

/// Nesting depths probed; serde_json's limit is 128.
pub const DEPTHS: [usize; 11] = [100, 127, 128, 129, 200, 255, 256, 1000, 1025, 10_000, 1_000_000];
/// Decode thread stacks: a small server worker's, and the main thread's.
pub const STACKS: [usize; 2] = [2 << 20, 8 << 20];
/// A probe running longer than this is recorded as a crash.
pub const TIMEOUT: Duration = Duration::from_mins(1);
/// Probes run at once. Low: a deep `Value` can take a gigabyte.
pub const PARALLEL: usize = 2;
/// The child's exit code for a rejected document.
pub const REJECTED: u8 = 10;

/// The container nested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `[[[...]]]`.
    Array,
    /// `{"a":{"a":...}}`.
    Object,
}

impl Shape {
    /// Stable identifier.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Array => "array",
            Self::Object => "object",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [Self::Array, Self::Object]
            .into_iter()
            .find(|shape| shape.name() == name)
    }
}

/// What the document is decoded into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// `serde_json::Value`.
    Value,
    /// `serde::de::IgnoredAny`.
    Ignored,
}

impl Decoded {
    /// Stable identifier.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Ignored => "ignored",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [Self::Value, Self::Ignored]
            .into_iter()
            .find(|target| target.name() == name)
    }
}

/// A document nested `depth` deep, innermost `null` for objects.
#[must_use]
pub fn document(shape: Shape, depth: usize) -> Vec<u8> {
    match shape {
        Shape::Array => [b"[".repeat(depth), b"]".repeat(depth)].concat(),
        Shape::Object => [br#"{"a":"#.repeat(depth), b"null".to_vec(), b"}".repeat(depth)].concat(),
    }
}

/// What a probe saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Exit 0.
    Accepted,
    /// Exit 10, with the status printed.
    Rejected(u16),
    /// Anything else.
    Crashed(String),
}

impl Probe {
    fn describe(&self) -> Value {
        match self {
            Self::Accepted => json!("accepted"),
            Self::Rejected(status) => json!(format!("rejected {status}")),
            Self::Crashed(detail) => json!(format!("crashed: {detail}")),
        }
    }
}

/// One probe's parameters.
#[derive(Debug, Clone, Copy)]
pub struct Job {
    /// Container.
    pub shape: Shape,
    /// Target.
    pub target: Decoded,
    /// Decode thread stack, in bytes.
    pub stack: usize,
    /// Nesting depth.
    pub depth: usize,
}

impl Job {
    fn series(self) -> String {
        format!("{}/{}/{}MiB", self.shape.name(), self.target.name(), self.stack >> 20)
    }
}

/// Every probe, grouped by series, depth ascending within one.
#[must_use]
pub fn jobs() -> Vec<Job> {
    let mut jobs = Vec::new();
    for shape in [Shape::Array, Shape::Object] {
        for target in [Decoded::Value, Decoded::Ignored] {
            for stack in STACKS {
                for depth in DEPTHS {
                    jobs.push(Job {
                        shape,
                        target,
                        stack,
                        depth,
                    });
                }
            }
        }
    }
    jobs
}

/// Run one probe of `backend` by re-executing `exe`.
#[must_use]
pub fn probe(exe: &Path, backend: &str, job: Job) -> Probe {
    let spawned = Command::new(exe)
        .args([
            "depth-probe",
            backend,
            job.shape.name(),
            &job.depth.to_string(),
            job.target.name(),
            &job.stack.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return Probe::Crashed(format!("could not spawn {}: {error}", exe.display())),
    };
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Probe::Crashed(format!("timed out after {}s", TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(error) => return Probe::Crashed(format!("wait failed: {error}")),
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let last = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    match status.code() {
        Some(0) => Probe::Accepted,
        Some(code) if code == i32::from(REJECTED) => stdout.trim().parse().map_or_else(
            |_| Probe::Crashed(format!("rejected with unreadable status {stdout:?}")),
            Probe::Rejected,
        ),
        Some(code) => Probe::Crashed(format!("exit code {code}: {last}")),
        None => Probe::Crashed(format!("{}: {last}", signal(status))),
    }
}

#[cfg(unix)]
fn signal(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    status
        .signal()
        .map_or_else(|| "killed".to_owned(), |signal| format!("signal {signal}"))
}

#[cfg(not(unix))]
fn signal(_: std::process::ExitStatus) -> String {
    "killed".to_owned()
}

/// Every probe of `backend`, [`PARALLEL`] at a time, in [`jobs`] order.
#[must_use]
pub fn probe_all(exe: &Path, backend: &str) -> Vec<Probe> {
    let jobs = jobs();
    let next = AtomicUsize::new(0);
    let mut results: Vec<Option<Probe>> = vec![None; jobs.len()];
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..PARALLEL)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&job) = jobs.get(index) else { break done };
                        done.push((index, probe(exe, backend, job)));
                    }
                })
            })
            .collect();
        for worker in workers {
            for (index, result) in worker.join().unwrap_or_default() {
                results[index] = Some(result);
            }
        }
    });
    results
        .into_iter()
        .map(|result| result.unwrap_or_else(|| Probe::Crashed("probe worker panicked".to_owned())))
        .collect()
}

/// serde_json's probes, once per process.
static REFERENCE: OnceLock<Vec<Probe>> = OnceLock::new();

/// Per series: the deepest accepted depth, and the first rejected and
/// crashed ones.
fn limits(probes: &[Probe]) -> Value {
    let mut series: BTreeMap<String, Value> = BTreeMap::new();
    for (job, probe) in jobs().into_iter().zip(probes) {
        let entry = series
            .entry(job.series())
            .or_insert_with(|| json!({ "max_accepted": null, "first_rejected": null, "first_crashed": null }));
        let slot = match probe {
            Probe::Accepted => "max_accepted",
            Probe::Rejected(_) => "first_rejected",
            Probe::Crashed(_) => "first_crashed",
        };
        if slot == "max_accepted" || entry[slot].is_null() {
            entry[slot] = json!(job.depth);
        }
    }
    json!(series)
}

/// Run the section, re-executing `exe`; without one it is skipped.
#[must_use]
pub fn run<B: Backend>(exe: Option<&Path>) -> Section {
    let mut section = Section::new("depth");
    let Some(exe) = exe else {
        section.note("skipped: no conform binary to re-execute");
        return section;
    };
    if !codecs::compiled().contains(&B::NAME) {
        section.note(format!(
            "skipped: {} is not a backend conform can dispatch by name",
            B::NAME
        ));
        return section;
    }
    let reference = REFERENCE.get_or_init(|| probe_all(exe, SerdeJson::NAME));
    let backend = if B::NAME == SerdeJson::NAME {
        reference.clone()
    } else {
        probe_all(exe, B::NAME)
    };
    for ((job, r), b) in jobs().into_iter().zip(reference).zip(&backend) {
        let case = format!("{}/{}", job.series(), job.depth);
        section.case();
        if r != b {
            section.differ(&case, r.describe(), b.describe());
        }
        match b {
            Probe::Crashed(detail) => section.fail(&case, format!("crashed: {detail}")),
            Probe::Rejected(422) => section.fail(&case, "too deep answered 422, not 400"),
            _ => {}
        }
    }
    section.data(
        "limits",
        json!({ "backend": limits(&backend), "reference": limits(reference) }),
    );
    section
}

/// The `depth-probe` subcommand: `<backend> <array|object> <depth>
/// <value|ignored> <stack_bytes>`. Returns the exit code.
#[must_use]
pub fn child(args: &[String]) -> u8 {
    let [backend, shape, depth, target, stack] = args else {
        return 2;
    };
    let (Some(shape), Ok(depth), Some(target), Ok(stack)) = (
        Shape::parse(shape),
        depth.parse(),
        Decoded::parse(target),
        stack.parse(),
    ) else {
        return 2;
    };
    disable_core_dumps();
    codecs::dispatch(
        backend,
        Child(Job {
            shape,
            target,
            stack,
            depth,
        }),
    )
    .unwrap_or(3)
}

struct Child(Job);

impl Visit for Child {
    type Output = u8;

    fn visit<B: Backend>(self) -> u8 {
        let Job {
            shape,
            target,
            stack,
            depth,
        } = self.0;
        let body = bytes::Bytes::from(document(shape, depth));
        let spawned = std::thread::Builder::new()
            .stack_size(stack)
            .spawn(move || match target {
                // Never dropped: `Value`'s recursive drop could overflow where the
                // parser did not, and that would not be the backend's crash.
                Decoded::Value => B::decode::<Value>(body).map(std::mem::forget),
                Decoded::Ignored => B::decode::<IgnoredAny>(body).map(drop),
            });
        match spawned.map(std::thread::JoinHandle::join) {
            Ok(Ok(Ok(()))) => 0,
            Ok(Ok(Err(failure))) => {
                println!("{}", failure.status());
                REJECTED
            }
            _ => 20,
        }
    }
}

/// An expected overflow should not leave a core dump behind.
#[cfg(unix)]
fn disable_core_dumps() {
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `none` is a valid rlimit that outlives the call, which only
    // reads it.
    unsafe {
        libc::setrlimit(libc::RLIMIT_CORE, &raw const none);
    }
}

#[cfg(not(unix))]
fn disable_core_dumps() {}
