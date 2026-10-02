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
use crate::matrix::{KYNOS, TIMED_SHAPES, variant};
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
    let rounds: u64 = flag(args, "--rounds").and_then(|v| v.parse().ok()).unwrap_or(ROUNDS);
    let core = flag(args, "--core").unwrap_or("2").to_owned();
    let trust = flag(args, "--trust").unwrap_or("solo").to_owned();
    anyhow::ensure!(
        matches!(trust.as_str(), "solo" | "canonical"),
        "--trust is solo or canonical"
    );
    let mut rows = Vec::new();
    for variant_name in filter.variants.clone().unwrap_or_default() {
        let variant = variant(&variant_name)?;
        let mut cells: Vec<(Cell, String, std::path::PathBuf)> = Vec::new();
        for set in filter.sets() {
            let release = build(Place::Host, "cell", set, &variant)?;
            for cell in filter.cells(set, TIMED_SHAPES) {
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
                    "time: round {}/{rounds} cell {}/{} {} {} {}{copy}",
                    round + 1,
                    n + 1,
                    order.len(),
                    cell.workload,
                    cell.shape.op,
                    cell.backend
                );
                let mut cmd = Command::new(cell_bin);
                cmd.arg("time").args(cell.args()).env("CELL_CORE", &core);
                if cfg!(target_os = "macos") {
                    // Tick-granular interference accounting needs longer windows.
                    cmd.env("CELL_SAMPLE_MS", "100").env("CELL_SAMPLES", "30");
                }
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
            let mut medians: Vec<f64> = results.iter().filter_map(|r| r["ns_median"].as_f64()).collect();
            medians.sort_by(f64::total_cmp);
            row["trust"] = json!(trust);
            row["rounds"] = json!(results);
            if medians.len() == results.len() && !medians.is_empty() {
                row["ns"] = json!(medians[medians.len() / 2]);
                row["ns_round_spread"] = json!(medians[medians.len() - 1] - medians[0]);
                row["discarded"] = json!(results.iter().filter_map(|r| r["discarded"].as_u64()).sum::<u64>());
            } else {
                row["error"] = json!("a round failed or was refused; see rounds");
            }
            rows.push(row);
        }
    }
    merge_rows(
        &root()
            .join("results")
            .join(crate::provenance::host_slug())
            .join("time.json"),
        &rows,
        "time",
    )
}
