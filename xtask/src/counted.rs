//! Counted measurements: callgrind (in the container), allocations and
//! hardware counters (on the host).

use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::build::{Place, bin, build};
use crate::matrix::{SETS, SHAPES, Set, Shape, Variant, label, variants};
use crate::util::{capture, parallel, root};

/// Cache geometry passed to callgrind explicitly, so the estimated-cycles
/// figure does not depend on which host simulated it: 32 KiB 8-way L1s and an
/// 8 MiB 16-way last level, 64-byte lines.
pub const CACHE: [&str; 3] = ["--I1=32768,8,64", "--D1=32768,8,64", "--LL=8388608,16,64"];

/// The only environment a counted process gets.
const COUNTED_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Give a process callgrind counts an environment of nothing but a fixed
/// `PATH`. The environment is copied onto the client's initial stack, so its
/// size moves the stack's alignment, and with it the instructions that
/// alignment-sensitive routines execute: struson's json-small decode counts
/// 14 731 or 14 743 instructions depending on the length of one unrelated
/// variable. Inheriting the environment made a count depend on whatever the
/// caller had set, down to whether the checkout was dirty.
pub fn counted_env(cmd: &mut Command) -> &mut Command {
    cmd.env_clear().env("PATH", COUNTED_PATH)
}

/// Estimated cycles from callgrind's events: `Ir + 5·L1 misses + 35·LL misses`.
#[must_use]
pub fn estimated_cycles(e: &Events) -> u64 {
    e.ir + 5 * (e.i1mr + e.d1mr + e.d1mw) + 35 * (e.ilmr + e.dlmr + e.dlmw)
}

/// callgrind's cache-simulation events.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct Events {
    pub ir: u64,
    pub dr: u64,
    pub dw: u64,
    pub i1mr: u64,
    pub d1mr: u64,
    pub d1mw: u64,
    pub ilmr: u64,
    pub dlmr: u64,
    pub dlmw: u64,
}

/// Parse the `events:` and `totals:` lines of a callgrind output file.
/// Trailing zero counts are omitted by callgrind, so missing fields are zero.
pub fn parse_callgrind(text: &str) -> Result<Events> {
    let events: Vec<&str> = text
        .lines()
        .find_map(|l| l.strip_prefix("events:"))
        .context("no events line")?
        .split_whitespace()
        .collect();
    let totals: Vec<u64> = text
        .lines()
        .find_map(|l| l.strip_prefix("totals:").or_else(|| l.strip_prefix("summary:")))
        .context("no totals line")?
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    let get = |name: &str| {
        events
            .iter()
            .position(|e| *e == name)
            .and_then(|i| totals.get(i).copied())
            .unwrap_or(0)
    };
    Ok(Events {
        ir: get("Ir"),
        dr: get("Dr"),
        dw: get("Dw"),
        i1mr: get("I1mr"),
        d1mr: get("D1mr"),
        d1mw: get("D1mw"),
        ilmr: get("ILmr"),
        dlmr: get("DLmr"),
        dlmw: get("DLmw"),
    })
}

/// One cell to run: a backend in its set's build, one workload, one shape.
#[derive(Debug, Clone)]
pub struct Cell {
    pub set: &'static Set,
    pub backend: &'static str,
    pub workload: &'static str,
    pub shape: Shape,
}

impl Cell {
    /// The `cell` arguments after the mode.
    #[must_use]
    pub fn args(&self) -> [&str; 5] {
        [
            self.backend,
            self.shape.op,
            self.workload,
            self.shape.form,
            self.shape.arrival,
        ]
    }

    /// The identifying fields of a result row.
    #[must_use]
    pub fn key(&self, variant: &str) -> Value {
        json!({
            "variant": variant,
            "set": self.set.name,
            "backend": label(self.set, self.backend),
            "workload": self.workload,
            "op": self.shape.op,
            "form": self.shape.form,
            "arrival": self.shape.arrival,
        })
    }
}

/// Filters a run to part of the matrix.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub sets: Option<Vec<String>>,
    pub workloads: Option<Vec<String>>,
    pub variants: Option<Vec<String>>,
    pub shapes: Option<Vec<Shape>>,
}

impl Filter {
    /// Parse `--sets a,b --workloads c --variants v` style arguments.
    pub fn parse(args: &[String]) -> Self {
        let list = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .map(|v| v.split(',').map(str::to_owned).collect::<Vec<_>>())
        };
        Self {
            sets: list("--sets"),
            workloads: list("--workloads"),
            variants: list("--variants"),
            shapes: None,
        }
    }

    pub fn sets(&self) -> Vec<&'static Set> {
        SETS.iter()
            .filter(|s| self.sets.as_ref().is_none_or(|w| w.iter().any(|n| n == s.name)))
            .collect()
    }

    pub fn variants(&self) -> Vec<Variant> {
        variants()
            .into_iter()
            .filter(|v| self.variants.as_ref().is_none_or(|w| w.iter().any(|n| n == v.name)))
            .collect()
    }

    pub fn cells(&self, set: &'static Set, shapes: &[Shape]) -> Vec<Cell> {
        let workloads: Vec<&'static str> = payloads::ALL
            .iter()
            .copied()
            .filter(|w| self.workloads.as_ref().is_none_or(|f| f.iter().any(|n| n == w)))
            .collect();
        let shapes = self.shapes.as_deref().unwrap_or(shapes);
        let mut cells = Vec::new();
        for backend in set.backends {
            for workload in &workloads {
                for shape in shapes {
                    cells.push(Cell {
                        set,
                        backend,
                        workload,
                        shape: *shape,
                    });
                }
            }
        }
        cells
    }
}

fn jobs(args: &[String], default: usize) -> usize {
    args.iter()
        .position(|a| a == "--jobs")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Parse a `cell` stdout line.
fn cell_json(stdout: &str) -> Value {
    stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or(Value::Null)
}

/// `xtask count`: callgrind over every cell at every valgrind-capable
/// variant. Must run inside the container (`xtask count` re-executes itself
/// there when it is not).
pub fn count(args: &[String]) -> Result<()> {
    if std::env::var_os("SJC_IN_CONTAINER").is_none() {
        return crate::container::reexec("count", args);
    }
    let filter = Filter::parse(args);
    let threads = jobs(args, 8);
    let mut rows = Vec::new();
    for variant in filter.variants().into_iter().filter(|v| v.valgrind) {
        for set in filter.sets() {
            let release = build(Place::Container, "cell", set, &variant)?;
            let cell_bin = bin(&release, "cell");
            let scratch = root().join("target/callgrind").join(variant.name).join(set.name);
            std::fs::create_dir_all(&scratch)?;
            let cells = filter.cells(set, SHAPES);
            eprintln!("count: {} {} ({} cells)", variant.name, set.name, cells.len());
            let jobs: Vec<Box<dyn FnOnce() -> Value + Send>> = cells
                .into_iter()
                .enumerate()
                .map(|(i, cell)| {
                    let cell_bin = cell_bin.clone();
                    let out_file = scratch.join(format!("{i}.out"));
                    let variant = variant.name;
                    Box::new(move || callgrind_cell(&cell_bin, &cell, &out_file, variant))
                        as Box<dyn FnOnce() -> Value + Send>
                })
                .collect();
            rows.extend(parallel(threads, jobs));
        }
    }
    let path = root()
        .join("results/callgrind")
        .join(format!("{}.json", std::env::consts::ARCH));
    if args.iter().any(|a| a == "--check") {
        let committed = crate::util::read_json(&path)?;
        let tolerance = tolerance(args)?;
        let compared = check_reproduced(&committed, &rows, tolerance)
            .with_context(|| format!("comparing with {}", path.display()))?;
        eprintln!(
            "count --check: {compared} counts reproduce within {} %",
            tolerance * 100.0
        );
        return Ok(());
    }
    merge_rows(&path, &rows, "callgrind")
}

/// Instruction counts may move this much between hosts before a rerun is
/// called irreproducible (it covers libc and kernel-version differences in
/// what the measured region calls, not noise: callgrind has none).
const REPRODUCIBLE: f64 = 0.005;

/// `--tolerance PCT`, as a fraction; [`REPRODUCIBLE`] when absent. A rerun on
/// the host that took the committed counts passes `--tolerance 0`: there,
/// every count must come back identical.
fn tolerance(args: &[String]) -> Result<f64> {
    let Some(i) = args.iter().position(|a| a == "--tolerance") else {
        return Ok(REPRODUCIBLE);
    };
    let pct: f64 = args
        .get(i + 1)
        .context("--tolerance needs a percentage")?
        .parse()
        .context("--tolerance needs a percentage")?;
    anyhow::ensure!(
        pct.is_finite() && pct >= 0.0,
        "--tolerance must be a non-negative percentage"
    );
    Ok(pct / 100.0)
}

/// `count --check`: compare fresh rows with the committed ones; returns how
/// many were compared. A committed count that the rerun failed to produce is
/// a failure too, not a row to skip.
fn check_reproduced(committed: &Value, fresh: &[Value], tolerance: f64) -> Result<usize> {
    let key = |r: &Value| {
        ["variant", "set", "backend", "workload", "op", "form", "arrival"]
            .map(|k| r[k].to_string())
            .join("|")
    };
    let old: std::collections::HashMap<String, f64> = committed["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((key(r), r["ir"].as_f64()?)))
        .collect();
    let mut compared = 0;
    let mut off = Vec::new();
    for row in fresh {
        let Some(old) = old.get(&key(row)) else {
            continue;
        };
        compared += 1;
        let Some(new) = row["ir"].as_f64() else {
            off.push(format!(
                "{}: {old:.0} -> no count ({})",
                key(row),
                row["error"].as_str().or(row["verdict"].as_str()).unwrap_or("no result")
            ));
            continue;
        };
        let drift = (new - old).abs() / old;
        if drift > tolerance {
            off.push(format!(
                "{}: {old:.0} -> {new:.0} ({:+.4} %)",
                key(row),
                (new / old - 1.0) * 100.0
            ));
        }
    }
    anyhow::ensure!(compared > 0, "no committed rows to compare against");
    if !off.is_empty() {
        anyhow::bail!(
            "{} of {compared} counts moved more than {} %:\n{}",
            off.len(),
            tolerance * 100.0,
            off.join("\n")
        );
    }
    Ok(compared)
}

fn callgrind_cell(cell_bin: &std::path::Path, cell: &Cell, out_file: &std::path::Path, variant: &str) -> Value {
    let mut row = cell.key(variant);
    let mut cmd = Command::new("valgrind");
    counted_env(&mut cmd)
        .args([
            "--tool=callgrind",
            "--collect-atstart=no",
            "--toggle-collect=*cell_measured*",
            "--cache-sim=yes",
        ])
        .args(CACHE)
        .arg(format!("--callgrind-out-file={}", out_file.display()))
        .arg(cell_bin)
        .arg("callgrind")
        .args(cell.args());
    match capture(&mut cmd) {
        Ok((ok, stdout, stderr)) => {
            let result = cell_json(&stdout);
            merge(&mut row, &result);
            if !ok && result.is_null() {
                row["error"] = json!(valgrind_error(&stderr));
            }
            if result["verdict"] == "exact" || result["verdict"] == "inexact" {
                let iters = result["iters"].as_u64().unwrap_or(1).max(1);
                match std::fs::read_to_string(out_file)
                    .map_err(anyhow::Error::from)
                    .and_then(|t| parse_callgrind(&t))
                {
                    Ok(events) => {
                        let per = |v: u64| v as f64 / iters as f64;
                        row["ir"] = json!(per(events.ir));
                        row["est_cycles"] = json!(per(estimated_cycles(&events)));
                        row["l1_misses"] = json!(per(events.i1mr + events.d1mr + events.d1mw));
                        row["ll_misses"] = json!(per(events.ilmr + events.dlmr + events.dlmw));
                    }
                    Err(e) => row["error"] = json!(format!("callgrind output: {e}")),
                }
            }
        }
        Err(e) => row["error"] = json!(e.to_string()),
    }
    let _ = std::fs::remove_file(out_file);
    row
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// Why valgrind failed a cell: the instruction it could not decode, when that
/// is the reason, else the end of its stderr. On an undecodable instruction
/// valgrind ends its output with the same bug-report boilerplate every time,
/// so the tail alone would not say which instruction or why.
fn valgrind_error(stderr: &str) -> String {
    let undecodable: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("unhandled instruction") || l.contains("Unrecognised instruction"))
        .map(str::trim)
        .take(2)
        .collect();
    if undecodable.is_empty() {
        tail(stderr)
    } else {
        undecodable.join("\n")
    }
}

fn merge(row: &mut Value, extra: &Value) {
    if let (Value::Object(row), Value::Object(extra)) = (row, extra) {
        for (k, v) in extra {
            if !row.contains_key(k) {
                row.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Replace, in the result file at `path`, every row matching a new row's key,
/// and stamp provenance. Partial reruns therefore update in place.
pub fn merge_rows(path: &std::path::Path, rows: &[Value], kind: &str) -> Result<()> {
    crate::util::merge_write(
        path,
        kind,
        rows,
        &["variant", "set", "backend", "workload", "op", "form", "arrival"],
        json!({}),
    )
}

/// `xtask alloc`: allocation counts on the host, every variant.
pub fn alloc(args: &[String]) -> Result<()> {
    host_mode(args, "alloc", SHAPES, &crate::provenance::host_slug(), None)
}

/// `xtask hw`: hardware instruction and cycle counters (Linux).
pub fn hw(args: &[String]) -> Result<()> {
    if !cfg!(target_os = "linux") {
        anyhow::bail!("hardware counters need perf_event_open, which only Linux has");
    }
    host_mode(args, "hw", SHAPES, &crate::provenance::host_slug(), Some(jobs(args, 4)))
}

fn host_mode(args: &[String], mode: &str, shapes: &[Shape], host: &str, threads: Option<usize>) -> Result<()> {
    let filter = Filter::parse(args);
    let threads = threads.unwrap_or_else(|| jobs(args, 8));
    let mut rows = Vec::new();
    for variant in filter.variants() {
        for set in filter.sets() {
            let release = build(Place::Host, "cell", set, &variant)?;
            let cell_bin = bin(&release, "cell");
            let cells = filter.cells(set, shapes);
            eprintln!("{mode}: {} {} ({} cells)", variant.name, set.name, cells.len());
            let jobs: Vec<Box<dyn FnOnce() -> Value + Send>> = cells
                .into_iter()
                .map(|cell| {
                    let cell_bin = cell_bin.clone();
                    let variant = variant.name;
                    let mode = mode.to_owned();
                    Box::new(move || {
                        let mut row = cell.key(variant);
                        match capture(Command::new(&cell_bin).arg(&mode).args(cell.args())) {
                            Ok((_, stdout, stderr)) => {
                                let result = cell_json(&stdout);
                                if result.is_null() {
                                    row["error"] = json!(tail(&stderr));
                                }
                                merge(&mut row, &result);
                            }
                            Err(e) => row["error"] = json!(e.to_string()),
                        }
                        row
                    }) as Box<dyn FnOnce() -> Value + Send>
                })
                .collect();
            rows.extend(parallel(threads, jobs));
        }
    }
    merge_rows(
        &root().join("results").join(host).join(format!("{mode}.json")),
        &rows,
        mode,
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Cell, REPRODUCIBLE, callgrind_cell, check_reproduced, tolerance, valgrind_error};
    use crate::matrix::{SETS, SHAPES};

    fn row(workload: &str, ir: Option<f64>) -> Value {
        let mut row = json!({
            "variant": "portable", "set": "baseline", "backend": "serde_json",
            "workload": workload, "op": "decode", "form": "owned", "arrival": "shared",
        });
        if let Some(ir) = ir {
            row["ir"] = json!(ir);
        } else {
            row["error"] = json!("valgrind died");
        }
        row
    }

    #[test]
    fn a_counted_process_sees_only_a_fixed_path() {
        let mut cmd = std::process::Command::new("env");
        cmd.env("SJC_GIT_DIRTY", "true");
        let out = super::counted_env(&mut cmd).output().unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("PATH={}\n", super::COUNTED_PATH)
        );
    }

    #[test]
    fn identical_counts_reproduce_at_zero_tolerance() {
        let committed = json!({ "rows": [row("json-small", Some(1000.0)), row("echo-post", Some(2000.0))] });
        let fresh = [row("json-small", Some(1000.0)), row("echo-post", Some(2000.0))];
        assert_eq!(check_reproduced(&committed, &fresh, 0.0).unwrap(), 2);
    }

    #[test]
    fn one_instruction_off_fails_at_zero_tolerance_but_not_at_the_default() {
        let committed = json!({ "rows": [row("json-small", Some(1000.0))] });
        let fresh = [row("json-small", Some(1001.0))];
        let err = check_reproduced(&committed, &fresh, 0.0).unwrap_err().to_string();
        assert!(err.contains("1 of 1 counts moved"), "{err}");
        assert_eq!(check_reproduced(&committed, &fresh, REPRODUCIBLE).unwrap(), 1);
    }

    #[test]
    fn a_count_the_rerun_lost_is_a_failure_not_a_skip() {
        let committed = json!({ "rows": [row("json-small", Some(1000.0))] });
        let err = check_reproduced(&committed, &[row("json-small", None)], REPRODUCIBLE)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no count (valgrind died)"), "{err}");
    }

    #[test]
    fn nothing_to_compare_is_a_failure() {
        let committed = json!({ "rows": [row("json-small", Some(1000.0))] });
        assert!(check_reproduced(&committed, &[row("echo-post", Some(1.0))], REPRODUCIBLE).is_err());
    }

    #[test]
    fn tolerance_is_a_percentage() {
        let args = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            tolerance(&args(&["--check"])).unwrap().to_bits(),
            REPRODUCIBLE.to_bits()
        );
        assert_eq!(
            tolerance(&args(&["--tolerance", "0"])).unwrap().to_bits(),
            0f64.to_bits()
        );
        assert!((tolerance(&args(&["--tolerance", "0.5"])).unwrap() - 0.005).abs() < 1e-12);
        assert!(tolerance(&args(&["--tolerance", "-1"])).is_err());
        assert!(tolerance(&args(&["--tolerance"])).is_err());
    }

    /// A cell that yields no counts still has its callgrind output removed:
    /// valgrind is absent (a spawn error) or cannot run the missing binary (a
    /// failed run), and either way the row records an error and the file goes.
    #[test]
    fn removes_the_callgrind_output_of_a_failed_cell() {
        let dir = std::env::temp_dir().join(format!("sjc-callgrind-cell-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out_file = dir.join("callgrind.out");
        std::fs::write(&out_file, "stale").unwrap();
        let cell = Cell {
            set: &SETS[0],
            backend: "serde_json",
            workload: "json-small",
            shape: SHAPES[0],
        };
        let row = callgrind_cell(&dir.join("no-such-cell"), &cell, &out_file, "portable");
        let removed = !out_file.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(row["error"].is_string(), "{row}");
        assert!(removed, "callgrind output survived a failed cell");
    }

    #[test]
    fn names_the_instruction_valgrind_could_not_decode() {
        let stderr = "\
==7== Callgrind, a call-graph generating cache profiler
disInstr(arm64): unhandled instruction 0x25F8C540
disInstr(arm64): 0010'0101 1111'1000 1100'0101 0100'0000
==7== valgrind: Unrecognised instruction at address 0x4a1f20.
==7==    at 0x4a1f20: std::rt::lang_start_internal (mod.rs:713)
==7== Process terminating with default action of signal 4 (SIGILL)
If that doesn't help, please report this bug to: www.valgrind.org
In the bug report, send all the above text, the valgrind
version, and what OS and version you are using.  Thanks.";
        assert_eq!(
            valgrind_error(stderr),
            "disInstr(arm64): unhandled instruction 0x25F8C540\n\
             ==7== valgrind: Unrecognised instruction at address 0x4a1f20."
        );
    }

    #[test]
    fn falls_back_to_the_tail() {
        let stderr = "a\nb\nc\nd\ne\nf\ng";
        assert_eq!(valgrind_error(stderr), "c\nd\ne\nf\ng");
    }
}
