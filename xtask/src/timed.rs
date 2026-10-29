//! Timed measurement: hardware cycles, which decide, and wall-clock time,
//! which corroborates, each stamped with how far to trust it.
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
use crate::counted::{Cell, Filter};
use crate::matrix::{KYNOS, Shape, TIMED_SHAPES, hw_shapes, variant};
use crate::util::{capture, root, shuffle};

/// Rounds per cell unless `--rounds` says otherwise; the report states any
/// run that took fewer.
pub const ROUNDS: u64 = 3;

/// The workloads timed by default: the decision shapes, the sweep, and the
/// real-world documents and their variants. Families are counted, not timed.
pub fn default_workloads() -> Vec<String> {
    struct Group;
    impl payloads::Visit for Group {
        type Output = payloads::Group;
        fn visit<W: payloads::Workload>(self) -> payloads::Group {
            W::GROUP
        }
    }
    payloads::ALL
        .iter()
        .filter(|w| {
            KYNOS.contains(w)
                || matches!(
                    payloads::dispatch(w, Group),
                    Some(payloads::Group::Sweep | payloads::Group::World)
                )
        })
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
    rounds(args, &filter, "time", &|_| TIMED_SHAPES, "ns_median", "ns")
}

/// `xtask hw`: hardware cycle and instruction counters at `native`, every
/// workload and shape, under the same protocol as [`time`]: one pinned
/// process at a time, rounds in shuffled order, an A/A copy of serde_json.
/// The cycles decide rule 1, so a run is `solo` unless it claims `canonical`,
/// which a loaded host is refused.
pub fn hw(args: &[String]) -> Result<()> {
    let mut filter = Filter::parse(args);
    if filter.variants.is_none() {
        filter.variants = Some(vec!["native".into()]);
    }
    rounds(args, &filter, "hw", &hw_shapes, "cycles", "cycles")
}

/// The trust class a run claims: `--trust`, else `SJC_TRUST` (so `mise run
/// all` can pass it to every timed task), else `solo`.
pub fn trust(args: &[String]) -> String {
    flag(args, "--trust")
        .map(str::to_owned)
        .or_else(|| std::env::var("SJC_TRUST").ok())
        .unwrap_or_else(|| "solo".into())
}

/// The 1-minute load average above which a run cannot be `canonical`.
const QUIET_LOAD: f64 = 1.0;

/// How long a `canonical` stage waits for the load to settle before it is
/// refused: long enough for the 1-minute average to decay after the stage
/// before it (fuzzing, on every core, in `mise run all`).
const SETTLE_LIMIT: std::time::Duration = std::time::Duration::from_mins(15);
/// How often the load is read while waiting.
const SETTLE_STEP: std::time::Duration = std::time::Duration::from_secs(15);

/// The 1-minute load average; unreadable counts as busy.
fn load_now() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|l| l.split_whitespace().next().and_then(|l| l.parse().ok()))
        .unwrap_or(f64::INFINITY)
}

/// Wait, at most `limit`, for `load` to fall to [`QUIET_LOAD`], reading it
/// every `step`; the last reading.
fn settle(
    load: &mut dyn FnMut() -> f64,
    pause: &mut dyn FnMut(std::time::Duration),
    limit: std::time::Duration,
    step: std::time::Duration,
) -> f64 {
    let mut waited = std::time::Duration::ZERO;
    loop {
        let now = load();
        if now <= QUIET_LOAD || waited >= limit {
            return now;
        }
        eprintln!(
            "canonical: waiting for the 1-minute load ({now}) to fall to {QUIET_LOAD} ({}s of at most {}s)",
            waited.as_secs(),
            limit.as_secs()
        );
        pause(step);
        waited += step;
    }
}

/// Whether a host is quiet: the 1-minute load at most [`QUIET_LOAD`] and
/// every CPU's governor `performance` (one CPU in another mode moves the
/// clock of whatever runs on it, and a host that reports none is not shown
/// to be pinned).
fn quiet(load: f64, governors: &[(usize, String)]) -> Result<()> {
    let off: Vec<String> = governors
        .iter()
        .filter(|(_, g)| g != "performance")
        .map(|(cpu, g)| format!("cpu{cpu} {g}"))
        .collect();
    anyhow::ensure!(
        load <= QUIET_LOAD && !governors.is_empty() && off.is_empty(),
        "--trust canonical needs a quiet host: load {load} (at most {QUIET_LOAD}), every CPU's governor performance ({})",
        if governors.is_empty() {
            "no CPU reports one".to_owned()
        } else if off.is_empty() {
            "all are".to_owned()
        } else {
            format!("not {}", off.join(", "))
        }
    );
    Ok(())
}

/// Refuse `canonical` on a host that is not quiet: a busy machine, or a
/// governor on any CPU that moves the clock under the measurement. A
/// `canonical` stage first waits, bounded by [`SETTLE_LIMIT`], for the load
/// the stage before it left to settle. Either way the load the stage starts
/// at is recorded, to be stamped beside the load it ends at.
pub fn check_trust(trust: &str) -> Result<()> {
    anyhow::ensure!(matches!(trust, "solo" | "canonical"), "--trust is solo or canonical");
    if trust == "canonical" {
        let load = settle(&mut load_now, &mut std::thread::sleep, SETTLE_LIMIT, SETTLE_STEP);
        quiet(load, &crate::provenance::governors())?;
    }
    crate::provenance::stage_started();
    Ok(())
}

/// The SMT siblings of `core`, other than itself, from sysfs
/// (`thread_siblings_list`, a list of CPUs and ranges such as `2,10` or
/// `2-3`).
pub fn siblings(core: &str) -> Vec<String> {
    let list = std::fs::read_to_string(format!(
        "/sys/devices/system/cpu/cpu{core}/topology/thread_siblings_list"
    ))
    .unwrap_or_default();
    cpu_list(list.trim())
        .into_iter()
        .map(|c| c.to_string())
        .filter(|c| c != core)
        .collect()
}

/// The CPUs in a sysfs CPU list (`0-3,8,10-11`).
pub fn cpu_list(list: &str) -> Vec<usize> {
    list.split(',')
        .filter(|p| !p.is_empty())
        .flat_map(|part| match part.split_once('-') {
            Some((a, b)) => match (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                (Ok(a), Ok(b)) => (a..=b).collect(),
                _ => Vec::new(),
            },
            None => part.trim().parse().map(|c| vec![c]).unwrap_or_default(),
        })
        .collect()
}

/// Busy and total time of `cpus` in `/proc/stat` ticks.
fn ticks(cpus: &[String]) -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let mut busy = 0;
    let mut total = 0;
    for cpu in cpus {
        let line = stat.lines().find(|l| l.starts_with(&format!("cpu{cpu} ")))?;
        // user nice system idle iowait irq softirq steal guest guest_nice;
        // guest time is already counted in user.
        let f: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .take(8)
            .filter_map(|v| v.parse().ok())
            .collect();
        let [user, nice, system, idle, iowait, irq, softirq, steal] = f[..] else {
            return None;
        };
        busy += user + nice + system + irq + softirq + steal;
        total += user + nice + system + idle + iowait + irq + softirq + steal;
    }
    Some((busy, total))
}

/// How busy `cpus` were over the next `window`, as a fraction. Ticks are
/// 10 ms, so the window is long enough for a steady neighbour to show; a
/// brief one the rounds and the A/A control absorb.
fn busy_over(cpus: &[String], window: std::time::Duration) -> f64 {
    if cpus.is_empty() {
        return 0.0;
    }
    let before = ticks(cpus);
    std::thread::sleep(window);
    match (before, ticks(cpus)) {
        (Some((b0, t0)), Some((b1, t1))) if t1 > t0 => (b1 - b0) as f64 / (t1 - t0) as f64,
        _ => 0.0,
    }
}

/// The most of the SMT sibling's time other work may take around a cell.
const MAX_SIBLING_BUSY: f64 = 0.10;
/// How long the sibling is watched before and after each cell. `/proc/stat`
/// counts in 10 ms ticks, so a window of 250 ms reads in 4 % steps, fine
/// enough for the 10 % limit (one in 100 ms would read in 10 % steps).
const SIBLING_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);
/// How many times a cell waits for its sibling to go quiet before it is
/// recorded as refused.
const SIBLING_TRIES: u32 = 30;

/// Run one pinned cell process, retrying while the core's SMT sibling is
/// busy before or after it. The row records the sibling's busiest reading.
fn pinned_cell(cmd: &mut Command, siblings: &[String]) -> Value {
    let mut busiest = 0.0f64;
    let fraction = |a: Option<(u64, u64)>, b: Option<(u64, u64)>| match (a, b) {
        (Some((b0, t0)), Some((b1, t1))) if t1 > t0 => (b1 - b0) as f64 / (t1 - t0) as f64,
        _ => 0.0,
    };
    for _ in 0..SIBLING_TRIES {
        let start = ticks(siblings);
        let before = busy_over(siblings, SIBLING_WINDOW);
        if before > MAX_SIBLING_BUSY {
            busiest = busiest.max(before);
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        let mut result = capture(cmd).map_or(Value::Null, |(_, out, _)| {
            out.lines()
                .rev()
                .find_map(|l| serde_json::from_str(l).ok())
                .unwrap_or(Value::Null)
        });
        let after = busy_over(siblings, SIBLING_WINDOW);
        // Over the whole span too, the cell's own run included: a cell long
        // enough to cover several ticks is checked while it runs.
        let span = if siblings.is_empty() {
            0.0
        } else {
            fraction(start, ticks(siblings))
        };
        let worst = before.max(after).max(span);
        if worst > MAX_SIBLING_BUSY {
            busiest = busiest.max(worst);
            continue;
        }
        if let Value::Object(o) = &mut result {
            o.insert("sibling_busy".into(), json!(worst));
        }
        return result;
    }
    // A cell that measures nothing (the backend rejects the input, or
    // answers wrongly) needs no quiet sibling to say so.
    let result = capture(cmd).map_or(Value::Null, |(_, out, _)| {
        out.lines()
            .rev()
            .find_map(|l| serde_json::from_str(l).ok())
            .unwrap_or(Value::Null)
    });
    if matches!(result["verdict"].as_str(), Some("rejected" | "unsupported" | "wrong")) {
        return result;
    }
    json!({ "error": format!("the SMT sibling stayed busy (up to {:.0} %) around every try", busiest * 100.0) })
}

/// Run every cell of `filter` once per round as its own pinned process, in
/// an order shuffled per round, with serde_json twice per round as the A/A
/// control. `field` is the per-round figure the cell reports; the row's
/// `out` is the median over rounds.
fn rounds(
    args: &[String],
    filter: &Filter,
    mode: &str,
    shapes: &dyn Fn(&str) -> &'static [Shape],
    field: &str,
    out: &str,
) -> Result<()> {
    let rounds: u64 = flag(args, "--rounds").and_then(|v| v.parse().ok()).unwrap_or(ROUNDS);
    let core = flag(args, "--core").unwrap_or("2").to_owned();
    crate::doctor::require()?;
    let trust = trust(args);
    check_trust(&trust)?;
    anyhow::ensure!(
        filter.sets().iter().any(|s| s.name == "baseline"),
        "{mode} compares every backend with serde_json in the same session, so the baseline set must run too"
    );
    let sibling = siblings(&core);
    let mut rows = Vec::new();
    for variant_name in filter.variants.clone().unwrap_or_default() {
        let variant = variant(&variant_name)?;
        let mut cells: Vec<(Cell, String, std::path::PathBuf)> = Vec::new();
        for set in filter.sets() {
            let release = build(Place::Host, "cell", set, &variant)?;
            for cell in filter.cells_by(set, shapes) {
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
                cmd.arg(mode).args(cell.args()).env("CELL_CORE", &core);
                per_round[i].push(pinned_cell(&mut cmd, &sibling));
            }
        }
        for ((cell, copy, _), results) in cells.iter().zip(per_round) {
            let mut row = cell.key(variant.name);
            if !copy.is_empty() {
                row["backend"] = json!(format!("{}{copy}", row["backend"].as_str().unwrap_or("")));
            }
            row["trust"] = json!(trust);
            row["core"] = json!(core);
            row["siblings"] = json!(sibling.join(","));
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
                .all(|r| matches!(r["verdict"].as_str(), Some("rejected" | "unsupported" | "wrong")))
            {
                // Not measured, by the backend's own answer (a rejection, or a
                // wrong result, which rule 2 reads): recorded as such.
            } else {
                row["error"] = json!("a round failed or was refused; see rounds");
            }
            row["rounds"] = json!(results);
            rows.push(row);
        }
    }
    crate::util::replace_write(
        &root()
            .join("results")
            .join(crate::provenance::host_slug())
            .join(format!("{mode}.json")),
        mode,
        &rows,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{quiet, settle};

    fn govs(g: &[&str]) -> Vec<(usize, String)> {
        g.iter().enumerate().map(|(i, g)| (i, (*g).to_owned())).collect()
    }

    /// Every CPU is checked, not only cpu0.
    #[test]
    fn a_quiet_host_needs_every_cpu_at_performance() {
        assert!(quiet(0.5, &govs(&["performance", "performance"])).is_ok());
        let err = quiet(0.5, &govs(&["performance", "powersave"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not cpu1 powersave"), "{err}");
        assert!(quiet(0.5, &[]).is_err());
        assert!(quiet(1.5, &govs(&["performance"])).is_err());
    }

    /// The wait ends as soon as the load settles, reading it each step.
    #[test]
    fn settling_stops_once_the_load_is_quiet() {
        let mut readings = [6.0, 3.0, 0.9, 0.5].into_iter();
        let mut paused = Vec::new();
        let load = settle(
            &mut || readings.next().unwrap_or(f64::INFINITY),
            &mut |d| paused.push(d),
            Duration::from_mins(1),
            Duration::from_secs(15),
        );
        assert!((load - 0.9).abs() < f64::EPSILON);
        assert_eq!(paused, vec![Duration::from_secs(15); 2]);
    }

    /// The wait is bounded: a load that never settles returns its last
    /// reading after the limit, for the gate to refuse.
    #[test]
    fn settling_gives_up_at_the_limit() {
        let mut paused = Duration::ZERO;
        let load = settle(
            &mut || 4.0,
            &mut |d| paused += d,
            Duration::from_mins(1),
            Duration::from_secs(15),
        );
        assert!((load - 4.0).abs() < f64::EPSILON);
        assert_eq!(paused, Duration::from_mins(1));
    }
}
