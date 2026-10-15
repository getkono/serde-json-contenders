//! Timed measurement: corroboration, stamped with how far to trust it.
//!
//! Every cell of a round runs as its own process, in an order shuffled per
//! round, so drift over a session spreads across backends instead of
//! landing on whichever ran last. serde_json runs twice per round as an A/A
//! control; the gap between its two copies is the noise floor a difference
//! must clear.

use std::process::Command;

use anyhow::Result;
use serde_json::{Value, json};

use crate::build::{Place, bin, build};
use crate::counted::{Cell, Filter, merge_rows};
use crate::matrix::{KYNOS, SHAPES, Shape, TIMED_SHAPES, variant};
use crate::util::{capture, root, shuffle};

/// Rounds per cell unless `--rounds` says otherwise; the report states any
/// run that took fewer.
pub const ROUNDS: u64 = 3;

/// The workloads timed by default: the decision shapes, the sweep, and the
/// real-world documents. Families are counted, not timed.
fn default_workloads() -> Vec<String> {
    payloads::ALL
        .iter()
        .filter(|w| KYNOS.contains(w) || w.starts_with("sweep-") || w.ends_with("-like"))
        .map(|w| (*w).to_owned())
        .collect()
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// `xtask time`.
pub fn time(args: &[String]) -> Result<()> {
    let mut filter = Filter::parse(args);
    if filter.workloads.is_none() {
        filter.workloads = Some(default_workloads());
    }
    if filter.variants.is_none() {
        filter.variants = Some(vec!["native".into()]);
    }
    rounds(args, &filter, "time", TIMED_SHAPES, "ns_median", "ns")
}

/// `xtask hw`: hardware cycle and instruction counters at `native`, every
/// workload and shape, under the same protocol as [`time`]: one pinned
/// process at a time, rounds in shuffled order, an A/A copy of serde_json.
/// The cycles decide rule 1, so a run on a loaded host is refused unless it
/// is declared `solo`.
pub fn hw(args: &[String]) -> Result<()> {
    let mut filter = Filter::parse(args);
    if filter.variants.is_none() {
        filter.variants = Some(vec!["native".into()]);
    }
    rounds(args, &filter, "hw", SHAPES, "cycles", "cycles")
}

/// The 1-minute load average above which a run cannot be `canonical`.
const QUIET_LOAD: f64 = 1.0;

/// Refuse `canonical` on a host that is not quiet: a busy machine, or a
/// governor that moves the clock under the measurement.
pub fn check_trust(trust: &str) -> Result<()> {
    anyhow::ensure!(matches!(trust, "solo" | "canonical"), "--trust is solo or canonical");
    if trust == "canonical" {
        let load: f64 = std::fs::read_to_string("/proc/loadavg")?
            .split_whitespace()
            .next()
            .and_then(|l| l.parse().ok())
            .unwrap_or(f64::INFINITY);
        let governor =
            std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").unwrap_or_default();
        anyhow::ensure!(
            load <= QUIET_LOAD && governor.trim() == "performance",
            "--trust canonical needs a quiet host: load {load} (at most {QUIET_LOAD}), governor {} (performance)",
            governor.trim()
        );
    }
    Ok(())
}

/// The SMT siblings of `core`, other than itself, from sysfs.
fn siblings(core: &str) -> String {
    std::fs::read_to_string(format!(
        "/sys/devices/system/cpu/cpu{core}/topology/thread_siblings_list"
    ))
    .unwrap_or_default()
    .trim()
    .split([',', '-'])
    .filter(|c| *c != core && !c.is_empty())
    .collect::<Vec<_>>()
    .join(",")
}

/// Run every cell of `filter` once per round as its own pinned process, in
/// an order shuffled per round, with serde_json twice per round as the A/A
/// control. `field` is the per-round figure the cell reports; the row's
/// `out` is the median over rounds.
fn rounds(args: &[String], filter: &Filter, mode: &str, shapes: &[Shape], field: &str, out: &str) -> Result<()> {
    let rounds: u64 = flag(args, "--rounds").and_then(|v| v.parse().ok()).unwrap_or(ROUNDS);
    let core = flag(args, "--core").unwrap_or("2").to_owned();
    let trust = flag(args, "--trust").unwrap_or("solo").to_owned();
    check_trust(&trust)?;
    let sibling = siblings(&core);
    let mut rows = Vec::new();
    for variant_name in filter.variants.clone().unwrap_or_default() {
        let variant = variant(&variant_name)?;
        let mut cells: Vec<(Cell, String, std::path::PathBuf)> = Vec::new();
        for set in filter.sets() {
            let release = build(Place::Host, "cell", set, &variant)?;
            for cell in filter.cells(set, shapes) {
                let copies: &[&str] = if set.name == "baseline" { &["", "#aa"] } else { &[""] };
                for copy in copies {
                    cells.push((cell.clone(), (*copy).to_owned(), bin(&release, "cell")));
                }
            }
        }
        let mut per_round: Vec<Vec<Value>> = vec![Vec::new(); cells.len()];
        for round in 0..rounds {
            let mut order: Vec<usize> = (0..cells.len()).collect();
            shuffle(&mut order, 0x7157 + round);
            for (n, &i) in order.iter().enumerate() {
                let (cell, copy, cell_bin) = &cells[i];
                eprintln!(
                    "{mode}: round {}/{rounds} cell {}/{} {} {} {}{copy}",
                    round + 1,
                    n + 1,
                    order.len(),
                    cell.workload,
                    cell.shape.op,
                    cell.backend
                );
                let mut cmd = Command::new(cell_bin);
                cmd.arg(mode)
                    .args(cell.args())
                    .env("CELL_CORE", &core)
                    .env("CELL_SIBLINGS", &sibling);
                let result = capture(&mut cmd).map_or(Value::Null, |(_, out, _)| {
                    out.lines()
                        .rev()
                        .find_map(|l| serde_json::from_str(l).ok())
                        .unwrap_or(Value::Null)
                });
                per_round[i].push(result);
            }
        }
        for ((cell, copy, _), results) in cells.iter().zip(per_round) {
            let mut row = cell.key(variant.name);
            if !copy.is_empty() {
                row["backend"] = json!(format!("{}{copy}", row["backend"].as_str().unwrap_or("")));
            }
            row["trust"] = json!(trust);
            row["core"] = json!(core);
            row["siblings"] = json!(sibling);
            if let Some(first) = results.first() {
                for k in ["verdict", "detail", "size", "iters"] {
                    if !first[k].is_null() {
                        row[k] = first[k].clone();
                    }
                }
            }
            let mut medians: Vec<f64> = results.iter().filter_map(|r| r[field].as_f64()).collect();
            medians.sort_by(f64::total_cmp);
            if medians.len() == results.len() && !medians.is_empty() {
                row[out] = json!(medians[medians.len() / 2]);
                row[format!("{out}_round_spread")] = json!(medians[medians.len() - 1] - medians[0]);
                if mode == "hw" {
                    let mut ins: Vec<f64> = results.iter().filter_map(|r| r["instructions"].as_f64()).collect();
                    ins.sort_by(f64::total_cmp);
                    row["instructions"] = json!(ins.get(ins.len() / 2));
                }
                row["discarded"] = json!(results.iter().filter_map(|r| r["discarded"].as_u64()).sum::<u64>());
            } else if results
                .iter()
                .all(|r| matches!(r["verdict"].as_str(), Some("rejected" | "unsupported")))
            {
                // Not measured, by the backend's own answer: recorded as such.
            } else {
                row["error"] = json!("a round failed or was refused; see rounds");
            }
            row["rounds"] = json!(results);
            rows.push(row);
        }
    }
    merge_rows(
        &root()
            .join("results")
            .join(crate::provenance::host_slug())
            .join(format!("{mode}.json")),
        &rows,
        mode,
    )
}
