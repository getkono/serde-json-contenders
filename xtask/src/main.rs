//! Every task this repository runs, behind `cargo xtask <task>`.
//!
//! Each task writes provenance-stamped JSON under `results/`; `report` is the
//! only writer of the README's results, and it reads nothing else.

mod build;
mod container;
mod counted;
mod matrix;
mod provenance;
mod util;

use anyhow::Result;

const HELP: &str = "\
cargo xtask <task> [--sets a,b] [--variants v,w] [--workloads x,y] [--jobs N]

  image         build the valgrind container image
  count         callgrind instructions and estimated cycles (container)
  alloc         allocations per operation
  hw            hardware instruction and cycle counters (Linux)
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((task, rest)) = args.split_first() else {
        eprint!("{HELP}");
        return Ok(());
    };
    match task.as_str() {
        "image" => container::image(),
        "count" => counted::count(rest),
        "alloc" => counted::alloc(rest),
        "hw" => counted::hw(rest),
        _ => {
            eprint!("{HELP}");
            anyhow::bail!("unknown task {task}")
        }
    }
}
