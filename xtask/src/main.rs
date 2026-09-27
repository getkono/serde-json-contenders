//! Every task this repository runs, behind `cargo xtask <task>`.
//!
//! Each task writes provenance-stamped JSON under `results/`; `report` is the
//! only writer of the README's results, and it reads nothing else.

mod build;
mod container;
mod cost;
mod counted;
mod differential;
mod doctor;
mod endtoend;
mod footprint;
mod matrix;
mod provenance;
mod timed;
mod util;

use anyhow::Result;

const HELP: &str = "\
cargo xtask <task> [--sets a,b] [--variants v,w] [--workloads x,y] [--jobs N]

  image         build the valgrind container image
  doctor        verify every build compiled and runs its claimed SIMD paths
  count         callgrind instructions and estimated cycles (container)
  alloc         allocations per operation
  hw            hardware instruction and cycle counters (Linux)
  time          timed sampling [--rounds 3] [--core 2] [--trust solo|canonical]
  e2e-count     callgrind per HTTP request, codec share (container)
  e2e-time      loopback throughput and open-loop p99 [--reps 5] [--seconds 30]
  size          .text of a fixture per backend, minus the floor
  compile-time  clean and incremental build times [--reps 5]
  msrv          does each backend build on Rust 1.85
  footprint     crates added, their unsafe counts, RustSec advisories
  conformance   the differential suite, every set at every variant
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((task, rest)) = args.split_first() else {
        eprint!("{HELP}");
        return Ok(());
    };
    match task.as_str() {
        "image" => container::image(),
        "doctor" => doctor::doctor(rest),
        "count" => counted::count(rest),
        "alloc" => counted::alloc(rest),
        "hw" => counted::hw(rest),
        "time" => timed::time(rest),
        "e2e-count" => endtoend::count(rest),
        "e2e-time" => endtoend::time(rest),
        "size" => cost::size(rest),
        "compile-time" => cost::compile_time(rest),
        "msrv" => cost::msrv(rest),
        "footprint" => footprint::footprint(rest),
        "conformance" => differential::conformance(rest),
        _ => {
            eprint!("{HELP}");
            anyhow::bail!("unknown task {task}")
        }
    }
}
