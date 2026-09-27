//! The decision rule, applied.
//!
//! Dominance is ε-dominance with ε = 5 %: `a` dominates `b` when `a` is no
//! more than 5 % worse than `b` on every axis and more than 5 % better on at
//! least one. Deterministic counts have no noise, so without a materiality
//! threshold a 0.1 % difference would decide a frontier; with one, compile
//! times (which are timed) cannot flip it either.

use serde_json::{Value, json};

use super::data::{Data, Key};
use crate::matrix::KYNOS;

/// Materiality: differences within this fraction are not differences.
pub const EPSILON: f64 = 0.05;
/// Rule 4's end-to-end threshold.
pub const E2E_GAIN: f64 = 0.10;

/// One measurable entry point.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub backend: &'static str,
    pub krate: &'static str,
    pub set: &'static str,
    /// Can be recommended for adoption (borrowed decode from a slice and a
    /// `to_vec`-style encoder, or decode-only by design).
    pub eligible: bool,
    pub encodes: bool,
}

/// Every entry point, baseline first.
pub const ENTRIES: &[Entry] = &[
    Entry {
        backend: "serde_json",
        krate: "serde_json",
        set: "baseline",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "serde_json+float_roundtrip",
        krate: "serde_json",
        set: "baseline-float-roundtrip",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "sonic-rs",
        krate: "sonic-rs",
        set: "sonic-rs",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "simd-json",
        krate: "simd-json",
        set: "simd-json",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "simd-json-buffers",
        krate: "simd-json",
        set: "simd-json",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "flexon-rt",
        krate: "flexon",
        set: "flexon-rt",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "flexon-rt-mut",
        krate: "flexon",
        set: "flexon-rt",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "flexon-ct",
        krate: "flexon",
        set: "flexon-ct",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "flexon-ct-mut",
        krate: "flexon",
        set: "flexon-ct",
        eligible: true,
        encodes: true,
    },
    Entry {
        backend: "jiter",
        krate: "jiter",
        set: "jiter",
        eligible: true,
        encodes: false,
    },
    Entry {
        backend: "hifijson",
        krate: "hifijson",
        set: "hifijson",
        eligible: false,
        encodes: false,
    },
    Entry {
        backend: "struson",
        krate: "struson",
        set: "struson",
        eligible: false,
        encodes: true,
    },
];

/// A rule's outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pass(String),
    Fail(String),
    Pending(String),
}

impl Outcome {
    pub fn word(&self) -> &'static str {
        match self {
            Self::Pass(_) => "pass",
            Self::Fail(_) => "fail",
            Self::Pending(_) => "pending",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Pass(s) | Self::Fail(s) | Self::Pending(s) => s,
        }
    }
}

/// Where a CPU figure comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub arch: &'static str,
    pub variant: &'static str,
    /// `callgrind` (estimated cycles) or `hw` (hardware cycles).
    pub source: &'static str,
}

impl Point {
    pub fn label(&self) -> String {
        format!(
            "{} {} ({})",
            self.arch,
            self.variant,
            if self.source == "hw" { "cycles" } else { "est. cycles" }
        )
    }
}

/// The points rule 1 reads, in order.
pub const X86_V3: Point = Point {
    arch: "x86_64",
    variant: "v3",
    source: "callgrind",
};
pub const X86_NATIVE: Point = Point {
    arch: "x86_64",
    variant: "native",
    source: "hw",
};
pub const X86_PORTABLE: Point = Point {
    arch: "x86_64",
    variant: "portable",
    source: "callgrind",
};
pub const ARM_NATIVE: Point = Point {
    arch: "aarch64",
    variant: "native",
    source: "callgrind",
};
pub const ARM_PORTABLE: Point = Point {
    arch: "aarch64",
    variant: "portable",
    source: "callgrind",
};
pub const POINTS: &[Point] = &[X86_PORTABLE, X86_V3, X86_NATIVE, ARM_PORTABLE, ARM_NATIVE];

/// CPU per operation at a point.
pub fn cpu(data: &Data, p: Point, backend: &str, workload: &str, op: &str) -> Option<f64> {
    let key = Key::new(p.variant, backend, workload, op);
    match p.source {
        "hw" => data
            .hosts
            .iter()
            .filter(|h| h.arch() == p.arch)
            .find_map(|h| h.hw.get(&key, "cycles")),
        _ => data.callgrind.get(p.arch)?.get(&key, "est_cycles"),
    }
}

/// Heap bytes per operation (allocation counts do not depend on the variant
/// in practice; the point's variant is preferred, native otherwise).
pub fn heap(data: &Data, variant: &str, backend: &str, workload: &str, op: &str) -> Option<f64> {
    let host = data.primary()?;
    host.alloc
        .get(&Key::new(variant, backend, workload, op), "bytes")
        .or_else(|| host.alloc.get(&Key::new("native", backend, workload, op), "bytes"))
}

fn set_row<'a>(rows: &'a [Value], set: &str, variant: Option<&str>) -> Option<&'a Value> {
    rows.iter()
        .find(|r| r["set"] == set && variant.is_none_or(|v| r["variant"] == v))
}

/// `.text` added, for the full or the decode-only fixture.
pub fn text(data: &Data, set: &str, variant: &str, full: bool) -> Option<f64> {
    let rows = &data.primary()?.size;
    let row = set_row(rows, set, Some(variant)).or_else(|| set_row(rows, set, Some("native")))?;
    row[if full { "full_delta" } else { "decode_delta" }].as_f64()
}

/// Clean release build seconds.
pub fn compile(data: &Data, set: &str) -> Option<f64> {
    set_row(&data.primary()?.compile, set, None)?["release_s"].as_f64()
}

/// The axis vector at a point for one Kynos (or any) workload.
fn axes(data: &Data, p: Point, e: &Entry, workload: &str, encode: bool) -> Vec<Option<f64>> {
    let mut v = vec![
        cpu(data, p, e.backend, workload, "decode"),
        heap(data, p.variant, e.backend, workload, "decode"),
    ];
    if encode {
        v.push(cpu(data, p, e.backend, workload, "encode"));
        v.push(heap(data, p.variant, e.backend, workload, "encode"));
    }
    v.push(text(data, e.set, p.variant, encode));
    v.push(compile(data, e.set));
    v
}

/// Whether `a` ε-dominates `b`, over axes both have.
fn dominates(a: &[Option<f64>], b: &[Option<f64>]) -> bool {
    let mut better = false;
    for (a, b) in a.iter().zip(b) {
        let (Some(a), Some(b)) = (a, b) else { continue };
        if *a > b * (1.0 + EPSILON) {
            return false;
        }
        if *a < b * (1.0 - EPSILON) {
            better = true;
        }
    }
    better
}

/// Whether `e` is on the frontier at `p` for `workload` among `pool`, and
/// the best material win over serde_json on a CPU or heap axis if so.
pub fn frontier_win(data: &Data, p: Point, e: &Entry, workload: &str, pool: &[&Entry]) -> Option<(String, f64)> {
    let encode = e.encodes && pool.iter().all(|o| o.encodes);
    let mine = axes(data, p, e, workload, encode);
    // A decode figure is required to say anything at all.
    mine[0]?;
    for other in pool {
        if other.backend != e.backend && dominates(&axes(data, p, other, workload, encode), &mine) {
            return None;
        }
    }
    let base = ENTRIES[0];
    let names = if encode {
        &["decode CPU", "decode heap", "encode CPU", "encode heap"][..]
    } else {
        &["decode CPU", "decode heap"][..]
    };
    let theirs = axes(data, p, &base, workload, encode);
    names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let (Some(m), Some(t)) = (mine[i], theirs[i]) else {
                return None;
            };
            (t > 0.0 && m < t * (1.0 - EPSILON)).then(|| ((*name).to_owned(), m / t))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// A library's verdict.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub entry: Entry,
    pub rules: [Outcome; 4],
    pub recommended: Outcome,
    pub exists: (bool, String),
}

fn rule1(data: &Data, e: &Entry) -> Outcome {
    if e.backend.starts_with("serde_json") {
        return Outcome::Pass("baseline".into());
    }
    let measured = KYNOS.iter().any(|w| {
        [X86_V3, X86_NATIVE]
            .iter()
            .any(|p| cpu(data, *p, e.backend, w, "decode").is_some())
    });
    if !measured {
        return Outcome::Pending("x86-64 counts not yet recorded".into());
    }
    let pool: Vec<&Entry> = ENTRIES.iter().filter(|x| x.eligible).collect();
    let mut x86 = None;
    let mut arm = None;
    let mut any_arm_data = false;
    for w in KYNOS {
        for p in [X86_V3, X86_NATIVE] {
            if x86.is_none() {
                x86 =
                    frontier_win(data, p, e, w, &pool).map(|(axis, r)| format!("{w} {axis} {r:.2}× at {}", p.label()));
            }
        }
        any_arm_data |= cpu(data, ARM_NATIVE, e.backend, w, "decode").is_some();
        if arm.is_none() {
            arm = frontier_win(data, ARM_NATIVE, e, w, &pool)
                .map(|(axis, r)| format!("{w} {axis} {r:.2}× at {}", ARM_NATIVE.label()));
        }
    }
    match (x86, arm) {
        (Some(x), Some(a)) => Outcome::Pass(format!("{x}; {a}")),
        (Some(x), None) if !any_arm_data => Outcome::Pending(format!("{x}; aarch64 counts not yet recorded")),
        (Some(x), None) => Outcome::Fail(format!("{x}, but no material frontier win on aarch64")),
        (None, _) => Outcome::Fail(
            "not on the frontier with a ≥5 % CPU or heap win on any Kynos shape at x86-64-v3 or native".into(),
        ),
    }
}

fn rule2(data: &Data, e: &Entry) -> Outcome {
    let mut runs = 0;
    let mut failures = Vec::new();
    for host in &data.hosts {
        for ((variant, set), report) in &host.conformance {
            let b = &report["backends"][e.backend];
            if b.is_null() {
                continue;
            }
            runs += 1;
            if b["gate"] != "pass" {
                let first = b["gating_failures"]
                    .as_array()
                    .and_then(|f| f.first())
                    .cloned()
                    .unwrap_or(Value::Null);
                failures.push(format!(
                    "{variant}/{set}: {} ({})",
                    first["section"].as_str().unwrap_or("?"),
                    first["case"].as_str().unwrap_or("?")
                ));
            }
        }
    }
    let set = e.set;
    let fuzz: Vec<&Value> = data.fuzz.iter().filter(|r| r["set"] == set).collect();
    for r in &fuzz {
        if r["divergence_found"] == true {
            failures.push(format!(
                "fuzz {} ({})",
                r["target"].as_str().unwrap_or("?"),
                r["config"].as_str().unwrap_or("?")
            ));
        }
    }
    let miri = data.miri.iter().find(|r| r["set"] == set);
    if miri.is_some_and(|r| r["outcome"] == "undefined-behavior") {
        failures.push("Miri reports undefined behavior".into());
    }
    if e.backend.starts_with("serde_json") {
        return Outcome::Pass("reference".into());
    }
    if !failures.is_empty() {
        failures.sort();
        failures.dedup();
        return Outcome::Fail(failures.into_iter().take(3).collect::<Vec<_>>().join("; "));
    }
    if runs == 0 {
        return Outcome::Pending("conformance not yet run".into());
    }
    if fuzz.is_empty() || miri.is_none() {
        return Outcome::Pending(format!("{runs} conformance runs agree; fuzzing or Miri not yet run"));
    }
    Outcome::Pass(format!(
        "{runs} conformance runs, {} fuzz targets, Miri {}",
        fuzz.len(),
        miri.map_or("?", |m| m["outcome"].as_str().unwrap_or("?"))
    ))
}

fn rule3(data: &Data, e: &Entry) -> Outcome {
    let Some(row) = data.footprint.iter().find(|r| r["set"] == e.set) else {
        return Outcome::Pending("footprint not yet recorded".into());
    };
    let mut problems = Vec::new();
    for c in row["crates"].as_array().into_iter().flatten() {
        for a in c["advisories"].as_array().into_iter().flatten() {
            if a["affects_pinned_version"] == true {
                problems.push(format!(
                    "{} {}",
                    a["id"].as_str().unwrap_or("?"),
                    c["name"].as_str().unwrap_or("?")
                ));
            }
        }
    }
    if let Some(open) = data
        .soundness
        .get(e.krate)
        .and_then(|t| t.get("open"))
        .and_then(toml::Value::as_array)
    {
        problems.extend(open.iter().filter_map(|v| v.as_str().map(str::to_owned)));
    }
    if problems.is_empty() {
        Outcome::Pass(format!(
            "no advisory at advisory-db {}",
            data.advisory_db.get(..8).unwrap_or("?")
        ))
    } else {
        Outcome::Fail(problems.join("; "))
    }
}

/// End-to-end gain over serde_json for one host and route.
pub fn e2e_gain(rows: &[Value], backend: &str, route: &str) -> Option<(f64, f64, f64, f64)> {
    let find = |b: &str| rows.iter().find(|r| r["backend"] == b && r["route"] == route);
    let (me, sj) = (find(backend)?, find("serde_json")?);
    let aa = find("serde_json#aa");
    let rps = me["rps"].as_f64()? / sj["rps"].as_f64()? - 1.0;
    let p99 = 1.0 - me["p99_ms"].as_f64().unwrap_or(f64::NAN) / sj["p99_ms"].as_f64().unwrap_or(f64::NAN);
    let aa_rps = aa
        .and_then(|a| Some((a["rps"].as_f64()? / sj["rps"].as_f64()? - 1.0).abs()))
        .unwrap_or(0.0);
    let aa_p99 = aa
        .and_then(|a| Some((1.0 - a["p99_ms"].as_f64()? / sj["p99_ms"].as_f64()?).abs()))
        .unwrap_or(0.0);
    Some((rps, p99, aa_rps, aa_p99))
}

/// The throughput gain the counted per-request cost predicts, CPU-bound.
pub fn e2e_predicted(data: &Data, arch: &str, backend: &str, route: &str) -> Option<f64> {
    let rows = data.e2e_count.get(arch)?;
    let variant = if arch == "aarch64" { "native" } else { "v3" };
    let get = |b: &str| {
        rows.iter()
            .find(|r| r["backend"] == b && r["route"] == route && r["variant"] == variant)
            .and_then(|r| r["est_cycles"].as_f64())
    };
    Some(get("serde_json")? / get(backend)? - 1.0)
}

fn rule4(data: &Data, e: &Entry) -> Outcome {
    if e.backend.starts_with("serde_json") {
        return Outcome::Pass("baseline".into());
    }
    let routes: &[&str] = if e.encodes {
        &["echo-post", "json-large-get", "json-large-post"]
    } else {
        &["json-large-post"]
    };
    let mut seen = false;
    let mut best: Option<String> = None;
    let mut inconsistent = Vec::new();
    for host in &data.hosts {
        for route in routes {
            let Some((rps, p99, aa_rps, aa_p99)) = e2e_gain(&host.e2e_time, e.backend, route) else {
                continue;
            };
            seen = true;
            let predicted = e2e_predicted(data, host.arch(), e.backend, route);
            let pass_rps = rps >= E2E_GAIN && rps > aa_rps;
            let pass_p99 = p99 >= E2E_GAIN && p99 > aa_p99;
            if pass_rps || pass_p99 {
                if predicted.is_some_and(|pr| rps > pr + EPSILON.max(aa_rps)) {
                    inconsistent.push(format!(
                        "{} {route}: +{:.0} % measured, +{:.0} % predicted",
                        host.slug,
                        rps * 100.0,
                        predicted.unwrap_or(0.0) * 100.0
                    ));
                    continue;
                }
                let trust = host.e2e_trust.clone().unwrap_or_default();
                best.get_or_insert(format!(
                    "{route} {:+.0} % rps, {:+.0} % p99 on {} ({trust})",
                    rps * 100.0,
                    p99 * 100.0,
                    host.slug
                ));
            }
        }
    }
    match (seen, best) {
        (false, _) => Outcome::Pending("end-to-end timing not yet recorded".into()),
        (true, Some(b)) if b.contains("(solo)") => {
            Outcome::Pending(format!("provisional pass: {b}; awaits a quiet-host rerun"))
        }
        (true, Some(b)) => Outcome::Pass(b),
        (true, None) if !inconsistent.is_empty() => Outcome::Fail(format!(
            "gains inconsistent with counted codec share: {}",
            inconsistent.join("; ")
        )),
        (true, None) => Outcome::Fail("no route ≥10 % better in throughput or p99 beyond the A/A band".into()),
    }
}

fn exists(data: &Data, e: &Entry) -> (bool, String) {
    if e.backend == "serde_json" {
        return (true, "the baseline".into());
    }
    let pool: Vec<&Entry> = ENTRIES.iter().collect();
    let mut best: Option<(f64, String)> = None;
    for p in POINTS {
        for w in payloads::ALL {
            if let Some((axis, r)) = frontier_win(data, *p, e, w, &pool)
                && best.as_ref().is_none_or(|(b, _)| r < *b)
            {
                best = Some((r, format!("{w} {axis} {r:.2}× serde_json at {}", p.label())));
            }
        }
    }
    let note = data
        .notes
        .get(e.backend)
        .and_then(|n| n.get("merit"))
        .and_then(toml::Value::as_str);
    match (best, note) {
        (Some((_, b)), Some(n)) => (true, format!("{b}. {n}")),
        (Some((_, b)), None) => (true, b),
        (None, Some(n)) => (true, n.to_owned()),
        (None, None) => (false, "no measured reason to choose it over serde_json".into()),
    }
}

/// Apply the rule to every entry.
pub fn judge(data: &Data) -> Vec<Verdict> {
    ENTRIES
        .iter()
        .map(|e| {
            let rules = [rule1(data, e), rule2(data, e), rule3(data, e), rule4(data, e)];
            let recommended = if !e.eligible {
                Outcome::Fail("reference point: no borrowed slice decode plus `to_vec`-style encoder".into())
            } else if let Some(f) = rules.iter().find(|r| matches!(r, Outcome::Fail(_))) {
                f.clone()
            } else if let Some(p) = rules.iter().find(|r| matches!(r, Outcome::Pending(_))) {
                p.clone()
            } else {
                Outcome::Pass("all four rules hold".into())
            };
            Verdict {
                entry: *e,
                rules,
                recommended,
                exists: exists(data, e),
            }
        })
        .collect()
}

/// Machine-readable verdicts.
pub fn to_json(verdicts: &[Verdict]) -> Value {
    json!({
        "schema": 1,
        "epsilon": EPSILON,
        "e2e_gain": E2E_GAIN,
        "entries": verdicts.iter().map(|v| json!({
            "backend": v.entry.backend,
            "crate": v.entry.krate,
            "recommended": v.recommended.word(),
            "reason": v.recommended.detail(),
            "rules": v.rules.iter().map(|r| json!({ "outcome": r.word(), "detail": r.detail() })).collect::<Vec<_>>(),
            "deserves_to_exist": v.exists.0,
            "exists_evidence": v.exists.1,
        })).collect::<Vec<_>>(),
    })
}
