//! The one binary behind every per-operation measurement.
//!
//! `cell <mode> <backend> <op> <workload> <form> <arrival> [iters]`
//!
//! Every mode first runs the operation once and checks the result; a cell
//! whose result is wrong is reported and never measured, so a backend cannot
//! be fast by doing less. Output is one JSON object on stdout.
//!
//! Modes: `check`, `callgrind` (run under valgrind by `xtask`), `alloc`,
//! `hw` (Linux hardware counters), `time`, `doctor`, `list`.

#[global_allocator]
static ALLOCATOR: alloc_count::Counting = alloc_count::Counting;

mod doctor;
mod job;
mod modes;
#[cfg(test)]
mod sensitivity;
mod verify;

use std::process::ExitCode;

use codecs::body::Arrival;
use serde_json::json;

use crate::job::{Form, Op, Spec};

fn usage() -> ExitCode {
    eprintln!(
        "usage: cell <check|callgrind|alloc|hw|time> <backend> <decode|encode|encode-into> <workload> <owned|borrowed> <shared|unique> [iters]\n       cell doctor | cell list"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("doctor") => {
            println!("{}", doctor::report());
            return ExitCode::SUCCESS;
        }
        Some("list") => {
            println!(
                "{}",
                json!({ "backends": codecs::compiled(), "workloads": payloads::ALL })
            );
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let [mode, backend, op, workload, form, arrival, rest @ ..] = args.as_slice() else {
        return usage();
    };
    let (Some(op), Some(form), Some(arrival)) = (Op::parse(op), Form::parse(form), parse_arrival(arrival)) else {
        return usage();
    };
    let iters = rest.first().and_then(|s| s.parse().ok());
    let spec = Spec {
        mode: mode.clone(),
        op,
        form,
        arrival,
        iters,
    };
    let Some(result) = codecs::dispatch(backend, job::ByBackend { spec, workload }) else {
        eprintln!(
            "backend {backend} is not compiled into this build: {:?}",
            codecs::compiled()
        );
        return ExitCode::from(3);
    };
    let Some(output) = result else {
        eprintln!("unknown workload {workload}");
        return ExitCode::from(3);
    };
    println!("{output}");
    if output["verdict"] == "wrong" || output["error"].is_string() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_arrival(s: &str) -> Option<Arrival> {
    match s {
        "shared" => Some(Arrival::Shared),
        "unique" => Some(Arrival::Unique),
        _ => None,
    }
}
