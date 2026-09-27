//! What each mode does with a job.

use std::hint::black_box;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::job::{Job, Spec, default_iters};

/// Verify, then measure as `spec.mode` says.
pub fn run(spec: &Spec, job: &mut impl Job) -> Value {
    let verdict = job.verify();
    let mut out = json!({ "verdict": verdict.name(), "size": job.size() });
    match &verdict {
        crate::verify::Verdict::Wrong(why) | crate::verify::Verdict::Rejected(why) => {
            out["detail"] = json!(why);
        }
        crate::verify::Verdict::Inexact { floats, max_ulps } => {
            out["inexact_floats"] = json!(floats);
            out["max_ulps"] = json!(max_ulps);
        }
        crate::verify::Verdict::Exact => {}
    }
    if !verdict.measurable() {
        return out;
    }
    let iters = spec.iters.unwrap_or_else(|| default_iters(job.size()));
    out["iters"] = json!(iters);
    let measured = match spec.mode.as_str() {
        "check" => json!({}),
        "callgrind" => callgrind(job, iters),
        "alloc" => alloc(job, iters),
        "hw" => hw(job, iters),
        "time" => time(job),
        other => json!({ "error": format!("unknown mode {other}") }),
    };
    if let (Value::Object(o), Value::Object(m)) = (&mut out, measured) {
        o.extend(m);
    }
    out
}

/// The measured region. Its name is what valgrind toggles collection on
/// (`--toggle-collect=*cell_measured*`); nothing else in the process is
/// counted.
#[inline(never)]
fn cell_measured<J: Job>(job: &mut J, iters: u64) {
    for _ in 0..black_box(iters) {
        job.run();
    }
}

fn callgrind(job: &mut impl Job, iters: u64) -> Value {
    // Warm: the first call pays for CPU-feature detection, lazy statics and
    // first-touch page faults, none of which a long-lived server pays again.
    job.prepare(1);
    job.run();
    job.prepare(iters);
    cell_measured(job, iters);
    json!({})
}

fn alloc(job: &mut impl Job, iters: u64) -> Value {
    job.prepare(1);
    job.run();
    job.prepare(iters);
    let before = alloc_count::Snapshot::now();
    alloc_count::reset_peak();
    cell_measured(job, iters);
    let after = alloc_count::Snapshot::now();
    let per = |a: u64, b: u64| (b - a) as f64 / iters as f64;
    json!({
        "allocations": per(before.allocations, after.allocations),
        "reallocations": per(before.reallocations, after.reallocations),
        "bytes": per(before.bytes, after.bytes),
        "peak_bytes": alloc_count::peak().saturating_sub(before.live),
        "leaked_bytes": after.live.saturating_sub(before.live),
    })
}

#[cfg(target_os = "linux")]
fn hw(job: &mut impl Job, iters: u64) -> Value {
    use perf_event::Builder;
    use perf_event::events::Hardware;

    let counters = (|| -> std::io::Result<_> {
        let mut group = perf_event::Group::builder()
            .exclude_kernel(true)
            .exclude_hv(true)
            .build_group()?;
        let builder = |event| {
            let mut b = Builder::new(event);
            b.exclude_kernel(true).exclude_hv(true);
            b
        };
        let instructions = group.add(&builder(Hardware::INSTRUCTIONS))?;
        let cycles = group.add(&builder(Hardware::CPU_CYCLES))?;
        Ok((group, instructions, cycles))
    })();
    let (mut group, instructions, cycles) = match counters {
        Ok(c) => c,
        Err(e) => return json!({ "error": format!("perf_event_open: {e}") }),
    };
    job.prepare(1);
    job.run();
    let reps = 101;
    let mut ins = Vec::with_capacity(reps);
    let mut cyc = Vec::with_capacity(reps);
    for _ in 0..reps {
        job.prepare(iters);
        let read = (|| -> std::io::Result<_> {
            group.reset()?;
            group.enable()?;
            cell_measured(job, iters);
            group.disable()?;
            group.read()
        })();
        match read {
            Ok(data) => {
                if data.time_running() != data.time_enabled() {
                    return json!({ "error": "counters were multiplexed; results would be scaled estimates" });
                }
                ins.push(data[&instructions] as f64 / iters as f64);
                cyc.push(data[&cycles] as f64 / iters as f64);
            }
            Err(e) => return json!({ "error": format!("perf read: {e}") }),
        }
    }
    json!({
        "instructions": median(&mut ins),
        "cycles": median(&mut cyc),
        "cycles_iqr": iqr(&mut cyc),
        "reps": reps,
    })
}

#[cfg(not(target_os = "linux"))]
fn hw(_: &mut impl Job, _: u64) -> Value {
    json!({ "error": "hardware counters are read through perf_event_open, which only Linux has" })
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn iqr(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() * 3 / 4] - v[v.len() / 4]
}

/// Timed sampling. Parameters are environment variables so `xtask` sets them
/// once for a whole sweep; the defaults are the protocol's.
fn time(job: &mut impl Job) -> Value {
    let env = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let warmup = Duration::from_millis(env("CELL_WARMUP_MS", 3000));
    let window = Duration::from_millis(env("CELL_SAMPLE_MS", 20));
    let samples = env("CELL_SAMPLES", 100) as usize;
    let max_other = env("CELL_MAX_OTHER_PERMILLE", 20) as f64 / 1000.0;
    let core = std::env::var("CELL_CORE").ok().and_then(|v| v.parse().ok());

    let pinned = core.map(crate::doctor::pin);
    // Calibrate a batch so one sample lasts about one window.
    job.prepare(1);
    let start = Instant::now();
    job.run();
    let mut batch: u64 = 1;
    loop {
        job.prepare(batch);
        let t = Instant::now();
        cell_measured(job, batch);
        if t.elapsed() >= window / 4 || batch >= 1 << 24 {
            let per = t.elapsed().as_secs_f64() / batch as f64;
            batch = ((window.as_secs_f64() / per) as u64).max(1);
            break;
        }
        batch *= 2;
    }
    while start.elapsed() < warmup {
        job.prepare(batch);
        cell_measured(job, batch);
    }
    let mut ns = Vec::with_capacity(samples);
    let mut discarded = 0u64;
    while ns.len() < samples {
        if discarded > 20 * samples as u64 {
            return json!({ "error": "host too busy: more than 20 discarded windows per kept one", "discarded": discarded });
        }
        job.prepare(batch);
        let probe = crate::doctor::Interference::start(core);
        let t = Instant::now();
        cell_measured(job, batch);
        let elapsed = t.elapsed();
        if probe.finish(elapsed) > max_other {
            discarded += 1;
            continue;
        }
        ns.push(elapsed.as_nanos() as f64 / batch as f64);
    }
    json!({
        "ns_median": median(&mut ns),
        "ns_iqr": iqr(&mut ns),
        "ns_min": ns.iter().copied().fold(f64::INFINITY, f64::min),
        "samples": samples,
        "batch": batch,
        "discarded": discarded,
        "pinned": pinned,
        "isolation": crate::doctor::Interference::method(),
    })
}
