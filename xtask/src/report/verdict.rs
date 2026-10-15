//! The decision rule, applied.
//!
//! Dominance is ε-dominance with ε = 5 %: `a` dominates `b` when `a` is no
//! more than 5 % worse than `b` on every axis and more than 5 % better on at
//! least one. Without a materiality threshold a 0.1 % difference would decide
//! a frontier; with one, compile times (which are timed) cannot flip it
//! either. Hardware cycles, which decide, must also clear the A/A band (the
//! distance between two copies of serde_json at that cell), and a CPU win
//! must be borne out by callgrind's deterministic count at v3.

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
    pub variant: &'static str,
    /// `hw` (hardware cycles) decides; `callgrind` (estimated cycles)
    /// corroborates and reproduces. `hw-instructions` (`instructions:u`) and
    /// `callgrind-ir` (instructions callgrind counted) are reported beside
    /// them.
    pub source: &'static str,
}

impl Point {
    pub fn label(&self) -> String {
        format!(
            "{} ({})",
            self.variant,
            match self.source {
                "hw" => "cycles",
                "hw-instructions" => "instructions",
                "callgrind-ir" => "Ir",
                _ => "est. cycles",
            }
        )
    }

    /// The point a [`frontier_win`] on `axis` was found at. A heap figure is
    /// read from the allocation counts, not from this point's CPU source, so
    /// a heap win names the build alone.
    pub fn win_label(&self, axis: &str) -> String {
        if axis.ends_with("heap") {
            self.variant.to_owned()
        } else {
            self.label()
        }
    }

    /// The same build, read as instructions instead of cycles.
    #[must_use]
    pub fn instructions(self) -> Self {
        Self {
            source: if self.source == "hw" {
                "hw-instructions"
            } else {
                "callgrind-ir"
            },
            ..self
        }
    }

    /// Whether this point's figures are timed, and so carry an A/A band.
    pub fn timed(self) -> bool {
        self.source.starts_with("hw")
    }
}

/// The point the rule reads: hardware cycles of the `native` build.
pub const X86_NATIVE: Point = Point {
    variant: "native",
    source: "hw",
};
/// The deterministic corroboration: callgrind's estimated cycles at
/// x86-64-v3 (valgrind cannot run the AVX-512 `native` build).
pub const X86_V3: Point = Point {
    variant: "v3",
    source: "callgrind",
};

/// CPU per operation at a point.
pub fn cpu(data: &Data, p: Point, backend: &str, workload: &str, op: &str) -> Option<f64> {
    let key = Key::new(p.variant, backend, workload, op);
    match p.source {
        "hw" => data.host.hw.get(&key, "cycles"),
        "hw-instructions" => data.host.hw.get(&key, "instructions"),
        "callgrind-ir" => data.callgrind.get(&key, "ir"),
        _ => data.callgrind.get(&key, "est_cycles"),
    }
}

/// How far serde_json's A/A copy lands from serde_json at this cell, as a
/// fraction: the noise a timed difference must clear. Zero for counts, which
/// have none.
pub fn aa_band(data: &Data, p: Point, workload: &str, op: &str) -> f64 {
    if !p.timed() {
        return 0.0;
    }
    match (
        cpu(data, p, "serde_json", workload, op),
        cpu(data, p, "serde_json#aa", workload, op),
    ) {
        (Some(a), Some(b)) if a > 0.0 => (b / a - 1.0).abs(),
        // A timed figure with no control cannot be told from noise.
        _ => f64::INFINITY,
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

/// One axis: the figure, if measured, and the fraction within which two
/// figures on it are not different (ε, or the A/A band where wider).
pub type Axis = (Option<f64>, f64);

/// The CPU axis tolerance at a point: ε, or the A/A band where wider.
pub fn cpu_tolerance(data: &Data, p: Point, workload: &str, op: &str) -> f64 {
    EPSILON.max(aa_band(data, p, workload, op))
}

/// The axis vector at a point for one workload.
fn axes(data: &Data, p: Point, e: &Entry, workload: &str, encode: bool) -> Vec<Axis> {
    let mut v = vec![
        (
            cpu(data, p, e.backend, workload, "decode"),
            cpu_tolerance(data, p, workload, "decode"),
        ),
        (heap(data, p.variant, e.backend, workload, "decode"), EPSILON),
    ];
    if encode {
        v.push((
            cpu(data, p, e.backend, workload, "encode"),
            cpu_tolerance(data, p, workload, "encode"),
        ));
        v.push((heap(data, p.variant, e.backend, workload, "encode"), EPSILON));
    }
    v.push((text(data, e.set, p.variant, encode), EPSILON));
    v.push((compile(data, e.set), EPSILON));
    v
}

/// Whether `a` dominates `b`, over axes both have: no more than the axis's
/// tolerance worse on any, and more than it better on at least one.
pub fn dominates(a: &[Axis], b: &[Axis]) -> bool {
    let mut better = false;
    for ((a, tol), (b, _)) in a.iter().zip(b) {
        let (Some(a), Some(b)) = (a, b) else { continue };
        if *a > b * (1.0 + tol) {
            return false;
        }
        if *a < b * (1.0 - tol) {
            better = true;
        }
    }
    better
}

/// Whether a CPU win at a timed point is borne out by the deterministic
/// count: at v3, `backend` costs no more than ε over serde_json on the same
/// cell. Noise can make a timed figure look like a win; it cannot move a
/// count.
pub fn corroborated(data: &Data, p: Point, backend: &str, workload: &str, op: &str) -> bool {
    if !p.timed() {
        return true;
    }
    match (
        cpu(data, X86_V3, backend, workload, op),
        cpu(data, X86_V3, "serde_json", workload, op),
    ) {
        (Some(m), Some(t)) => m <= t * (1.0 + EPSILON),
        _ => false,
    }
}

/// Whether `e` is on the frontier at `p` for `workload` among `pool`, and
/// the best material win over serde_json on a CPU or heap axis if so.
pub fn frontier_win(data: &Data, p: Point, e: &Entry, workload: &str, pool: &[&Entry]) -> Option<(String, f64)> {
    // An encoder is weighed on encode axes too, and only against encoders; a
    // decode-only entry is weighed on decode axes against everyone.
    let encode = e.encodes;
    let mine = axes(data, p, e, workload, encode);
    // A decode figure is required to say anything at all.
    mine[0].0?;
    for other in pool {
        if other.backend != e.backend
            && (!encode || other.encodes)
            && dominates(&axes(data, p, other, workload, encode), &mine)
        {
            return None;
        }
    }
    let base = ENTRIES[0];
    let names: &[(&str, Option<&str>)] = if encode {
        &[
            ("decode CPU", Some("decode")),
            ("decode heap", None),
            ("encode CPU", Some("encode")),
            ("encode heap", None),
        ]
    } else {
        &[("decode CPU", Some("decode")), ("decode heap", None)]
    };
    let theirs = axes(data, p, &base, workload, encode);
    names
        .iter()
        .enumerate()
        .filter_map(|(i, (name, cpu_op))| {
            let ((Some(m), tol), (Some(t), _)) = (mine[i], theirs[i]) else {
                return None;
            };
            let material = t > 0.0 && m < t * (1.0 - tol);
            let real = cpu_op.is_none_or(|op| corroborated(data, p, e.backend, workload, op));
            (material && real).then(|| ((*name).to_owned(), m / t))
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

fn rule1(data: &Data, e: &Entry, pool: &[&Entry]) -> Outcome {
    if e.backend == "serde_json" {
        return Outcome::Pass("baseline".into());
    }
    let measured = KYNOS
        .iter()
        .any(|w| cpu(data, X86_NATIVE, e.backend, w, "decode").is_some());
    if !measured {
        return Outcome::Pending("hardware cycles not yet recorded".into());
    }
    let best = KYNOS
        .iter()
        .filter_map(|w| frontier_win(data, X86_NATIVE, e, w, pool).map(|(axis, r)| (*w, axis, r)))
        .min_by(|a, b| a.2.total_cmp(&b.2));
    match best {
        Some((w, axis, r)) => {
            let win = format!("{w} {axis} {r:.2}× at {}", X86_NATIVE.win_label(&axis));
            // Allocation counts are not timed, so their trust class is moot.
            if axis.ends_with("heap") {
                return Outcome::Pass(win);
            }
            match hw_trust(data) {
                Some("canonical") => Outcome::Pass(win),
                trust => Outcome::Pending(format!(
                    "provisional pass: {win} ({}); awaits a quiet-host rerun",
                    trust.unwrap_or("?")
                )),
            }
        }
        None => Outcome::Fail(
            "not on the frontier with a CPU or heap win beyond ε and the A/A band, corroborated at v3, on any Kynos shape"
                .into(),
        ),
    }
}

/// The trust class the hardware counters were taken under.
pub fn hw_trust(data: &Data) -> Option<&str> {
    data.host.hw.rows.values().next().and_then(|r| r["trust"].as_str())
}

fn rule2(data: &Data, e: &Entry) -> Outcome {
    let mut runs = 0;
    // The same first failure recurs at every variant and build of a backend,
    // so each is stated once with the number of runs it gated.
    let mut gated: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    {
        for report in data.host.conformance.values() {
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
                *gated
                    .entry(format!(
                        "{} {}",
                        first["section"].as_str().unwrap_or("?"),
                        first["case"].as_str().unwrap_or("?")
                    ))
                    .or_default() += 1;
            }
        }
    }
    let mut failures: Vec<String> = gated
        .into_iter()
        .map(|(case, n)| format!("{case} fails {n} of {runs} conformance runs"))
        .collect();
    let set = e.set;
    let fuzz: Vec<&Value> = data.fuzz.iter().filter(|r| r["set"] == set).collect();
    let mut divergences: Vec<String> = fuzz
        .iter()
        .filter(|r| r["divergence_found"] == true)
        .map(|r| {
            format!(
                "fuzz {} ({})",
                r["target"].as_str().unwrap_or("?"),
                r["config"].as_str().unwrap_or("?")
            )
        })
        .collect();
    divergences.sort();
    divergences.dedup();
    let miri = data.miri.iter().find(|r| r["set"] == set);
    if let Some(r) = miri.filter(|r| r["outcome"] == "undefined-behavior") {
        // "error: Undefined Behavior: <what>: <where>" -> "<what>".
        let what = r["detail"]
            .as_str()
            .and_then(|d| d.split("Undefined Behavior: ").nth(1))
            .and_then(|d| d.split(':').next())
            .map(str::trim)
            .filter(|w| !w.is_empty());
        failures.push(what.map_or_else(
            || "Miri reports undefined behavior".to_owned(),
            |w| format!("Miri reports undefined behavior ({w})"),
        ));
    }
    if e.backend.starts_with("serde_json") {
        return Outcome::Pass("reference".into());
    }
    // Conformance cases and Miri are always stated in full; fuzz divergences
    // fill what room is left of three entries and the rest are counted.
    let room = 3usize.saturating_sub(failures.len());
    let dropped = divergences.len().saturating_sub(room);
    failures.extend(divergences.into_iter().take(room));
    if dropped > 0 {
        failures.push(format!(
            "{dropped} more fuzz divergence{}",
            if dropped == 1 { "" } else { "s" }
        ));
    }
    if !failures.is_empty() {
        return Outcome::Fail(failures.join("; "));
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
    // A memory error the sanitizer caught while fuzzing is a soundness defect
    // whatever else is recorded, so it fails before a missing footprint.
    let mut problems: Vec<String> = data
        .fuzz
        .iter()
        .filter(|r| r["set"] == e.set && r["sanitizer_report"].is_string())
        .map(|r| {
            format!(
                "sanitizer report fuzzing {} ({})",
                r["target"].as_str().unwrap_or("?"),
                r["config"].as_str().unwrap_or("?")
            )
        })
        .collect();
    let Some(row) = data.footprint.iter().find(|r| r["set"] == e.set) else {
        return if problems.is_empty() {
            Outcome::Pending("footprint not yet recorded".into())
        } else {
            Outcome::Fail(problems.join("; "))
        };
    };
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
pub fn e2e_predicted(data: &Data, backend: &str, route: &str) -> Option<f64> {
    let get = |b: &str| {
        data.e2e_count
            .iter()
            .find(|r| r["backend"] == b && r["route"] == route && r["variant"] == X86_V3.variant)
            .and_then(|r| r["est_cycles"].as_f64())
    };
    Some(get("serde_json")? / get(backend)? - 1.0)
}

fn rule4(data: &Data, e: &Entry) -> Outcome {
    if e.backend == "serde_json" {
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
    {
        let host = &data.host;
        for route in routes {
            let Some((rps, p99, aa_rps, aa_p99)) = e2e_gain(&host.e2e_time, e.backend, route) else {
                continue;
            };
            seen = true;
            let predicted = e2e_predicted(data, e.backend, route);
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

/// Whether `e` deserves to exist. Its frontier is drawn among `pool`: every
/// entry, eligible or not, that has not failed conformance. A win is read at
/// `native` hardware cycles (or heap) over every workload, gated as rule 1's
/// is, and a CPU win is stated with its v3 count beside it.
fn exists(data: &Data, e: &Entry, pool: &[&Entry]) -> (bool, String) {
    if e.backend == "serde_json" {
        return (true, "the baseline".into());
    }
    let mut best: Option<(f64, String)> = None;
    for w in payloads::ALL {
        if let Some((axis, r)) = frontier_win(data, X86_NATIVE, e, w, pool)
            && best.as_ref().is_none_or(|(b, _)| r < *b)
        {
            let op = axis.split(' ').next().unwrap_or("decode");
            let count = match (
                axis.ends_with("CPU"),
                cpu(data, X86_V3, e.backend, w, op),
                cpu(data, X86_V3, "serde_json", w, op),
            ) {
                (true, Some(m), Some(t)) if t > 0.0 => format!(" ({:.2}× at {})", m / t, X86_V3.label()),
                _ => String::new(),
            };
            best = Some((
                r,
                format!("{w} {axis} {r:.2}× serde_json at {}{count}", X86_NATIVE.win_label(&axis)),
            ));
        }
    }
    let note = super::notes::merit(data, e.backend);
    match (best, note) {
        (Some((_, b)), Some(n)) => (true, format!("{b}. {n}")),
        (Some((_, b)), None) => (true, b),
        (None, Some(n)) => (true, n),
        (None, None) => (false, "no measured reason to choose it over serde_json".into()),
    }
}

/// The entries that have not failed conformance. A backend that returns
/// wrong answers cannot define the frontier: rule 1's pool is the eligible
/// ones among these, and "deserves to exist" draws its own from all of them.
pub fn conforming(data: &Data) -> Vec<&'static Entry> {
    ENTRIES
        .iter()
        .filter(|x| !matches!(rule2(data, x), Outcome::Fail(_)))
        .collect()
}

/// Apply the rule to every entry.
pub fn judge(data: &Data) -> Vec<Verdict> {
    let conforming = conforming(data);
    let pool: Vec<&Entry> = conforming.iter().copied().filter(|x| x.eligible).collect();
    ENTRIES
        .iter()
        .map(|e| {
            let rules = [rule1(data, e, &pool), rule2(data, e), rule3(data, e), rule4(data, e)];
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
                exists: exists(data, e, &conforming),
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

#[cfg(test)]
pub(super) mod tests {
    use serde_json::json;

    use super::super::data::Key;
    use super::{Data, ENTRIES, Entry, Outcome, rule2, rule3};

    fn sonic() -> &'static Entry {
        ENTRIES.iter().find(|e| e.set == "sonic-rs").unwrap_or(&ENTRIES[0])
    }

    fn simd() -> &'static Entry {
        ENTRIES.iter().find(|e| e.backend == "simd-json").unwrap_or(&ENTRIES[0])
    }

    fn with_fuzz(rows: Vec<serde_json::Value>, footprint: bool) -> Data {
        Data {
            fuzz: rows,
            footprint: if footprint {
                vec![json!({ "set": "sonic-rs", "crates": [] })]
            } else {
                Vec::new()
            },
            ..Data::default()
        }
    }

    /// Hardware cycles at native and callgrind counts at v3 for json-small
    /// decode, per backend: `(backend, cycles, est_cycles)`.
    pub(in crate::report) fn measured(cells: &[(&str, f64, f64)], trust: &str) -> Data {
        let mut data = Data::default();
        data.host.slug = "x86_64-linux-amd-x".into();
        for (backend, cycles, est) in cells {
            data.host.hw.rows.insert(
                Key::new("native", backend, "json-small", "decode"),
                json!({ "cycles": cycles, "trust": trust }),
            );
            data.callgrind.rows.insert(
                Key::new("v3", backend, "json-small", "decode"),
                json!({ "est_cycles": est }),
            );
        }
        data
    }

    #[test]
    fn a_sanitizer_report_fails_rule3_even_without_a_footprint() {
        let e = sonic();
        let row = json!({ "set": e.set, "config": "portable", "target": "decode_value",
            "divergence_found": false, "sanitizer_report": "==1==ERROR: AddressSanitizer: SEGV" });
        for footprint in [false, true] {
            assert_eq!(
                rule3(&with_fuzz(vec![row.clone()], footprint), e),
                Outcome::Fail("sanitizer report fuzzing decode_value (portable)".into())
            );
        }
    }

    /// sonic-rs decodes json-small in half serde_json's cycles but fails
    /// conformance; simd-json takes 80 %. A wrong answer cannot decide which
    /// codecs are faster, so simd-json is on the frontier of "deserves to
    /// exist" as it is of rule 1.
    #[test]
    fn an_entry_that_failed_conformance_dominates_nobody() {
        let mut data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 101.0, 100.0),
                ("simd-json", 80.0, 90.0),
                ("sonic-rs", 50.0, 50.0),
            ],
            "canonical",
        );
        data.host.conformance.insert(
            ("native".into(), "sonic-rs".into()),
            json!({ "backends": { "sonic-rs": { "gate": "fail",
                "gating_failures": [{ "section": "grammar", "case": "n_x.json" }] } } }),
        );
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == "simd-json").unwrap();
        assert!(simd.exists.0, "{:?}", simd.exists);
        assert_eq!(
            simd.exists.1,
            "json-small decode CPU 0.80× serde_json at native (cycles) (0.90× at v3 (est. cycles))"
        );
        assert_eq!(
            simd.rules[0],
            Outcome::Pass("json-small decode CPU 0.80× at native (cycles)".into())
        );
    }

    /// Identical code measured 0.65× once (unpinned, on a loaded host). A
    /// timed win the A/A control cannot tell from noise is no win.
    #[test]
    fn a_cycles_win_inside_the_aa_band_is_noise() {
        let data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 140.0, 100.0),
                ("simd-json", 65.0, 100.0),
            ],
            "canonical",
        );
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == "simd-json").unwrap();
        assert!(matches!(simd.rules[0], Outcome::Fail(_)), "{:?}", simd.rules[0]);
    }

    /// A cycles win the deterministic count contradicts (the backend costs
    /// 30 % more at v3) is not borne out.
    #[test]
    fn a_cycles_win_the_count_contradicts_is_not_a_win() {
        let data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 100.0, 100.0),
                ("simd-json", 70.0, 130.0),
            ],
            "canonical",
        );
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == simd().backend).unwrap();
        assert!(matches!(simd.rules[0], Outcome::Fail(_)), "{:?}", simd.rules[0]);
        assert!(!simd.exists.0, "{:?}", simd.exists);
    }

    #[test]
    fn a_solo_cycles_win_is_provisional() {
        let data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 100.0, 100.0),
                ("simd-json", 70.0, 80.0),
            ],
            "solo",
        );
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == "simd-json").unwrap();
        assert_eq!(
            simd.rules[0],
            Outcome::Pending(
                "provisional pass: json-small decode CPU 0.70× at native (cycles) (solo); awaits a quiet-host rerun"
                    .into()
            )
        );
    }

    /// simd-json ties serde_json on decode cycles and allocates half its
    /// bytes. The heap figure comes from the allocation counts, so the win
    /// names the build without a cycles source.
    #[test]
    fn a_heap_win_claims_no_cycles_source() {
        let mut data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 100.0, 100.0),
                ("simd-json", 100.0, 100.0),
            ],
            "solo",
        );
        for (backend, bytes) in [("serde_json", 100.0), ("simd-json", 50.0)] {
            data.host.alloc.rows.insert(
                Key::new("native", backend, "json-small", "decode"),
                json!({ "bytes": bytes }),
            );
        }
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == "simd-json").unwrap();
        assert_eq!(simd.exists.1, "json-small decode heap 0.50× serde_json at native");
        assert_eq!(
            simd.rules[0],
            Outcome::Pass("json-small decode heap 0.50× at native".into())
        );
    }

    #[test]
    fn a_divergence_fails_rule2_not_rule3() {
        let e = sonic();
        let row = json!({ "set": e.set, "config": "v3", "target": "roundtrip",
            "divergence_found": true, "sanitizer_report": null });
        let data = with_fuzz(vec![row], true);
        assert!(matches!(rule3(&data, e), Outcome::Pass(_)));
        assert_eq!(rule2(&data, e), Outcome::Fail("fuzz roundtrip (v3)".into()));
    }

    #[test]
    fn a_conformance_failure_is_stated_once_with_its_run_count() {
        let e = sonic();
        let failing = json!({ "backends": { e.backend: { "gate": "fail",
            "gating_failures": [{ "section": "grammar", "case": "n_x.json" }] } } });
        let passing = json!({ "backends": { e.backend: { "gate": "pass" } } });
        let mut data = Data {
            miri: vec![json!({ "set": e.set, "outcome": "undefined-behavior",
                "detail": "error: Undefined Behavior: in-bounds pointer arithmetic failed: attempting to offset" })],
            ..Data::default()
        };
        for (variant, report) in [("native", &failing), ("portable", &failing), ("v3", &passing)] {
            data.host
                .conformance
                .insert((variant.into(), e.set.into()), report.clone());
        }
        assert_eq!(
            rule2(&data, e),
            Outcome::Fail(
                "grammar n_x.json fails 2 of 3 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed)"
                    .into()
            )
        );
    }

    #[test]
    fn fuzz_divergences_never_push_out_a_conformance_case_or_miri() {
        let e = sonic();
        let failing = json!({ "backends": { e.backend: { "gate": "fail",
            "gating_failures": [{ "section": "status", "case": "shape/x" }] } } });
        let diverged = |target: &str, config: &str| {
            json!({ "set": e.set, "config": config, "target": target,
                "divergence_found": true, "sanitizer_report": null })
        };
        let mut data = Data {
            fuzz: vec![
                diverged("decode_struct", "portable"),
                diverged("decode_struct", "v3"),
                diverged("decode_value", "v3"),
                diverged("roundtrip", "v3"),
            ],
            miri: vec![json!({ "set": e.set, "outcome": "undefined-behavior",
                "detail": "error: Undefined Behavior: in-bounds pointer arithmetic failed: attempting to offset" })],
            ..Data::default()
        };
        data.host.conformance.insert(("native".into(), e.set.into()), failing);
        assert_eq!(
            rule2(&data, e),
            Outcome::Fail(
                "status shape/x fails 1 of 1 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (portable); 3 more fuzz divergences"
                    .into()
            )
        );
    }
}
