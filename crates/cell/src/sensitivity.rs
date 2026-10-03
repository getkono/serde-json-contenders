//! The measurements can fail: an adapter that does more work shows exactly
//! that work, and one that does less is refused before it is measured.
//!
//! The counting allocator is process-wide, so the allocation test relies on
//! a process of its own, which `cargo nextest` (`mise run test`) gives every
//! test.
//!
//! The callgrind test needs valgrind and is ignored; it runs in the container:
//! `cargo test -p cell --locked -- --ignored sensitivity`.

use std::hint::black_box;
use std::path::Path;
use std::process::Command;

use bytes::Bytes;
use codecs::backend::serde_json::{SerdeJson, failure};
use codecs::body::Arrival;
use codecs::{Backend, Body, Failure, Visit};
use payloads::Workload;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::job::{ByBackend, Form, Job, Op, Spec};
use crate::verify::Verdict;

/// serde_json behind an adapter that copies the body once first, as an
/// in-place parser must copy a body it shares.
#[derive(Debug)]
struct CloneOnce;

impl Backend for CloneOnce {
    const NAME: &'static str = "clone-once";
    const CRATE: &'static str = "serde_json";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = false;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        let copy = Bytes::copy_from_slice(&body);
        drop(body);
        SerdeJson::decode(black_box(copy))
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        SerdeJson::decode_borrowed(body)
    }

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Failure> {
        SerdeJson::encode(value)
    }

    fn encode_into<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        SerdeJson::encode_into(value, out)
    }
}

/// Does less than it should, as a variant the optimizer had partly deleted
/// would: it decodes only the first element of every array, and encodes
/// nothing at all.
#[derive(Debug)]
struct DoesLess;

fn first_only(value: &mut Value) {
    match value {
        Value::Array(items) => {
            items.truncate(1);
            items.iter_mut().for_each(first_only);
        }
        Value::Object(members) => members.values_mut().for_each(first_only),
        _ => {}
    }
}

fn decode_less<'de, T: Deserialize<'de>>(input: &[u8]) -> Result<T, Failure> {
    let mut value: Value = serde_json::from_slice(input).map_err(|e| failure(&e))?;
    first_only(&mut value);
    T::deserialize(value).map_err(|e| failure(&e))
}

impl Backend for DoesLess {
    const NAME: &'static str = "does-less";
    const CRATE: &'static str = "serde_json";
    const ENCODES: bool = true;
    const ELIGIBLE: bool = false;

    fn decode<T: DeserializeOwned>(body: Bytes) -> Result<T, Failure> {
        decode_less(&body)
    }

    fn decode_borrowed<'a, T: Deserialize<'a>>(body: &'a mut Body) -> Result<T, Failure> {
        decode_less(body.as_slice())
    }

    fn encode<T: Serialize>(_: &T) -> Result<Vec<u8>, Failure> {
        Ok(Vec::new())
    }

    fn encode_into<T: Serialize>(_: &T, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.clear();
        Ok(())
    }
}

/// Only the copy [`CloneOnce`] adds: what its extra cost should be.
struct CopyOnly {
    body: Bytes,
}

impl Job for CopyOnly {
    fn prepare(&mut self, _: u64) {}

    fn run(&mut self) {
        black_box(Bytes::copy_from_slice(black_box(&self.body)));
    }

    fn verify(&mut self) -> Verdict {
        Verdict::Exact
    }

    fn size(&self) -> usize {
        self.body.len()
    }
}

fn spec(mode: &str, op: Op, iters: Option<u64>) -> Spec {
    Spec {
        mode: mode.to_owned(),
        op,
        form: Form::Owned,
        arrival: Arrival::Shared,
        iters,
    }
}

/// What the `cell` binary prints for `mode` on backend `B`.
fn cell<B: Backend>(mode: &str, op: Op, workload: &str, iters: Option<u64>) -> Value {
    ByBackend {
        spec: spec(mode, op, iters),
        workload,
    }
    .visit::<B>()
    .unwrap_or_else(|| panic!("no workload {workload}"))
}

fn field(output: &Value, name: &str) -> f64 {
    output[name].as_f64().unwrap_or_else(|| panic!("no {name} in {output}"))
}

/// Fields of an `alloc` cell that only grow when something else allocates.
const ALLOC_FIELDS: [&str; 5] = ["allocations", "reallocations", "bytes", "peak_bytes", "leaked_bytes"];

/// The smallest of each allocation field across `outputs`.
///
/// libtest's main thread records the test it just spawned (a map insert and
/// a queue push) while the test already runs, and the counter is
/// process-wide: on a loaded runner those allocations land inside a
/// measurement. They happen once per process, so a repeated measurement
/// misses them; a field's smallest value is the operation's own.
fn least(outputs: &[Value]) -> Value {
    let mut out = outputs[0].clone();
    for name in ALLOC_FIELDS {
        let min = outputs.iter().map(|o| field(o, name)).fold(f64::INFINITY, f64::min);
        out[name] = serde_json::json!(min);
    }
    out
}

#[test]
fn a_copy_of_the_body_is_one_more_allocation_of_its_size() {
    let (mut bases, mut clones) = (Vec::new(), Vec::new());
    for _ in 0..3 {
        bases.push(cell::<SerdeJson>("alloc", Op::Decode, "json-small", None));
        clones.push(cell::<CloneOnce>("alloc", Op::Decode, "json-small", None));
    }
    let (base, clone) = (least(&bases), least(&clones));
    let size = field(&base, "size");
    let more = |name: &str| field(&clone, name) - field(&base, name);
    assert!((more("allocations") - 1.0).abs() < 1e-9, "{base}\n{clone}");
    assert!(more("reallocations").abs() < 1e-9, "{base}\n{clone}");
    assert!((more("bytes") - size).abs() < 1e-9, "{base}\n{clone}");
    // The copy is live while the value is built from it.
    assert!((more("peak_bytes") - size).abs() < 1e-9, "{base}\n{clone}");
    assert!(field(&clone, "leaked_bytes").abs() < 1e-9, "{clone}");
}

#[test]
fn an_adapter_that_does_less_is_refused_not_measured() {
    for mode in ["check", "callgrind", "alloc", "hw", "time"] {
        for op in [Op::Decode, Op::Encode, Op::EncodeInto] {
            let out = cell::<DoesLess>(mode, op, "json-large", None);
            assert_eq!(out["verdict"], "wrong", "{mode} {op:?}: {out}");
            assert!(out.get("iters").is_none(), "{mode} {op:?} was measured: {out}");
        }
    }
    // The same workload through serde_json itself is measured.
    let out = cell::<SerdeJson>("check", Op::Decode, "json-large", None);
    assert_eq!(out["verdict"], "exact", "{out}");
    assert!(out.get("iters").is_some(), "{out}");
}

/// Iterations each callgrind helper runs: enough that per-call overhead is
/// a rounding error.
const COUNTED_ITERS: u64 = 64;

#[test]
#[ignore = "run under callgrind by a_copy_of_the_body_costs_its_instructions"]
fn counted_serde_json() {
    cell::<SerdeJson>("callgrind", Op::Decode, "json-large", Some(COUNTED_ITERS));
}

#[test]
#[ignore = "run under callgrind by a_copy_of_the_body_costs_its_instructions"]
fn counted_clone_once() {
    cell::<CloneOnce>("callgrind", Op::Decode, "json-large", Some(COUNTED_ITERS));
}

#[test]
#[ignore = "run under callgrind by a_copy_of_the_body_costs_its_instructions"]
fn counted_copy_only() {
    let (body, _rest) = Arrival::Shared.materialize(&payloads::kynos::JsonLarge::bytes());
    crate::modes::run(
        &spec("callgrind", Op::Decode, Some(COUNTED_ITERS)),
        &mut CopyOnly { body },
    );
}

/// Ir per iteration of one helper, counted as `xtask count` counts a cell:
/// only inside `cell_measured`.
fn ir_per_iteration(exe: &Path, helper: &str) -> f64 {
    let out = exe.with_file_name(format!("sensitivity-{helper}.callgrind"));
    let status = Command::new("valgrind")
        .args([
            "--tool=callgrind",
            "--collect-atstart=no",
            "--toggle-collect=*cell_measured*",
        ])
        .arg(format!("--callgrind-out-file={}", out.display()))
        .arg(exe)
        .args([
            "--exact",
            &format!("sensitivity::{helper}"),
            "--ignored",
            "--test-threads=1",
        ])
        .status()
        .expect("valgrind runs; this test needs the container");
    assert!(status.success(), "{helper} under valgrind: {status}");
    let text = std::fs::read_to_string(&out).expect("callgrind wrote its output");
    let _ = std::fs::remove_file(&out);
    let events: Vec<&str> = text
        .lines()
        .find_map(|l| l.strip_prefix("events:"))
        .expect("an events line")
        .split_whitespace()
        .collect();
    let totals: Vec<f64> = text
        .lines()
        .find_map(|l| l.strip_prefix("totals:").or_else(|| l.strip_prefix("summary:")))
        .expect("a totals line")
        .split_whitespace()
        .map(|v| v.parse().expect("a count"))
        .collect();
    let ir = events.iter().position(|e| *e == "Ir").expect("an Ir event");
    totals[ir] / COUNTED_ITERS as f64
}

#[test]
#[ignore = "needs valgrind; run in the container with `-- --ignored sensitivity`"]
fn a_copy_of_the_body_costs_its_instructions() {
    let exe = std::env::current_exe().expect("this test binary");
    let base = ir_per_iteration(&exe, "counted_serde_json");
    let clone = ir_per_iteration(&exe, "counted_clone_once");
    let copy = ir_per_iteration(&exe, "counted_copy_only");
    let size = payloads::kynos::JsonLarge::bytes().len() as f64;
    eprintln!("Ir per decode: serde_json {base:.0}, clone-once {clone:.0}; the copy alone {copy:.0}, of {size} bytes");
    // The copy did real work on every byte, at most 64 bytes an instruction.
    assert!(copy > size / 64.0, "{copy:.0} Ir cannot have copied {size} bytes");
    // And the copy is what the adapter added, give or take the call around it.
    let extra = clone - base;
    assert!(
        (extra - copy).abs() <= copy * 0.02,
        "clone-once added {extra:.0} Ir; the copy alone costs {copy:.0}"
    );
}
