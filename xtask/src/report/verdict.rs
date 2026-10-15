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
    /// Whether a timed axis is weighed with the A/A band (or ε alone, to ask
    /// whether only the band of `solo` timing hides a win).
    pub band: bool,
}

impl Point {
    /// The same point, with timed figures weighed at ε alone.
    #[must_use]
    pub fn without_band(self) -> Self {
        Self { band: false, ..self }
    }

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
    band: true,
};
/// The deterministic corroboration: callgrind's estimated cycles at
/// x86-64-v3 (valgrind cannot run the AVX-512 `native` build).
pub const X86_V3: Point = Point {
    variant: "v3",
    source: "callgrind",
    band: true,
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
/// fraction: the noise a timed difference must clear. That is the widest of
/// the gap between the medians and the gap in each round (the two run in the
/// same round, so a round's pair is one sample of the noise). Zero for
/// counts, which have none; `None` for a timed cell whose control is
/// missing, which cannot be told from noise at all.
pub fn aa_band(data: &Data, p: Point, workload: &str, op: &str) -> Option<f64> {
    if !p.timed() {
        return Some(0.0);
    }
    let (Some(a), Some(b)) = (
        cpu(data, p, "serde_json", workload, op),
        cpu(data, p, "serde_json#aa", workload, op),
    ) else {
        return None;
    };
    if a <= 0.0 {
        return None;
    }
    let field = if p.source == "hw-instructions" {
        "instructions"
    } else {
        "cycles"
    };
    let rounds = |backend: &str| -> Vec<Option<f64>> {
        data.host
            .hw
            .row(&Key::new(p.variant, backend, workload, op))
            .and_then(|r| r["rounds"].as_array())
            .map(|rs| rs.iter().map(|r| r[field].as_f64()).collect())
            .unwrap_or_default()
    };
    let per_round = rounds("serde_json")
        .into_iter()
        .zip(rounds("serde_json#aa"))
        .filter_map(|(x, y)| Some((y? / x.filter(|x| *x > 0.0)? - 1.0).abs()))
        .fold(0.0, f64::max);
    Some((b / a - 1.0).abs().max(per_round))
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

/// One axis of an entry: its figure, if measured; the fraction within which
/// two figures on it are not different (ε, or the A/A band where wider); and,
/// on a timed cycles axis, the same cell's deterministic count at v3, which
/// any advantage on the axis must agree with.
#[derive(Debug, Clone, Copy)]
pub struct Axis {
    pub value: Option<f64>,
    pub tol: f64,
    pub count: Count,
}

/// The deterministic count behind an axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Count {
    /// The axis is not timed: nothing to agree with.
    Untimed,
    /// The same cell's estimated cycles at v3.
    At(f64),
    /// A timed axis with no count: no advantage on it counts.
    Missing,
}

impl Axis {
    fn plain(value: Option<f64>) -> Self {
        Self {
            value,
            tol: EPSILON,
            count: Count::Untimed,
        }
    }
}

/// The CPU axis tolerance at a point: ε, or the A/A band where wider; `None`
/// without a control.
pub fn cpu_tolerance(data: &Data, p: Point, workload: &str, op: &str) -> Option<f64> {
    aa_band(data, p, workload, op).map(|b| if p.band { EPSILON.max(b) } else { EPSILON })
}

/// The cycles axis of `backend` at a point. Without an A/A control the
/// figure cannot be weighed, so it is treated as unmeasured.
pub fn cpu_axis(data: &Data, p: Point, backend: &str, workload: &str, op: &str) -> Axis {
    let count = if p.timed() {
        cpu(data, X86_V3, backend, workload, op).map_or(Count::Missing, Count::At)
    } else {
        Count::Untimed
    };
    match cpu_tolerance(data, p, workload, op) {
        Some(tol) => Axis {
            value: cpu(data, p, backend, workload, op),
            tol,
            count,
        },
        None => Axis {
            value: None,
            tol: EPSILON,
            count,
        },
    }
}

/// The axis vector at a point for one workload.
fn axes(data: &Data, p: Point, e: &Entry, workload: &str, encode: bool) -> Vec<Axis> {
    let mut v = vec![
        cpu_axis(data, p, e.backend, workload, "decode"),
        Axis::plain(heap(data, p.variant, e.backend, workload, "decode")),
    ];
    if encode {
        v.push(cpu_axis(data, p, e.backend, workload, "encode"));
        v.push(Axis::plain(heap(data, p.variant, e.backend, workload, "encode")));
    }
    v.push(Axis::plain(text(data, e.set, p.variant, encode)));
    v.push(Axis::plain(compile(data, e.set)));
    v
}

/// Whether the deterministic counts point the same way as a timed win of
/// `x` over `y`: `x`'s count is lower. A count no lower leaves a timed win
/// unexplained by the code, so it is not taken as one.
fn count_lower(x: &Axis, y: &Axis) -> bool {
    match (x.count, y.count) {
        (Count::Untimed, _) | (_, Count::Untimed) => true,
        (Count::At(a), Count::At(b)) => a < b,
        _ => false,
    }
}

/// Whether a timed axis lacks its count on either side.
fn count_missing(x: &Axis, y: &Axis) -> bool {
    matches!(x.count, Count::Missing) || matches!(y.count, Count::Missing)
}

/// Whether `a` dominates `b`: no more than the axis's tolerance worse on any
/// axis, and more than it better on at least one. On a cycles axis the two
/// entries' own counts at v3 must agree with each advantage: `a` being
/// faster counts only if its count is lower than `b`'s. Noise can make a
/// timed figure look like a win; it cannot move a count. A timed loss
/// beyond the tolerance always holds `a` back, whatever the counts say: the
/// counts are of another build, and cannot excuse a measured loss. An axis
/// `b` was measured on and `a` was not (a rejected input, a failed cell), or
/// a timed axis without both counts, is one `a` cannot be shown no worse
/// on, so `a` does not dominate.
pub fn dominates(a: &[Axis], b: &[Axis]) -> bool {
    let mut better = false;
    for (x, y) in a.iter().zip(b) {
        match (x.value, y.value) {
            (None, Some(_)) => return false,
            (Some(_), Some(_)) if count_missing(x, y) => return false,
            (Some(av), Some(bv)) => {
                if av > bv * (1.0 + x.tol) {
                    return false;
                }
                if av < bv * (1.0 - x.tol) && count_lower(x, y) {
                    better = true;
                }
            }
            _ => {}
        }
    }
    better
}

/// Whether `e` is non-dominated at `p` for `workload` among `pool`: what
/// rule 1's frontier means, without the further demand of a material win
/// over serde_json.
pub fn on_frontier(data: &Data, p: Point, e: &Entry, workload: &str, pool: &[&Entry]) -> bool {
    let encode = e.encodes;
    let mine = axes(data, p, e, workload, encode);
    mine[0].value.is_some()
        && !pool.iter().any(|other| {
            other.backend != e.backend
                && (!encode || other.encodes)
                && dominates(&axes(data, p, other, workload, encode), &mine)
        })
}

/// Whether `e` is on the frontier at `p` for `workload` among `pool`, and
/// the best material win over serde_json on a CPU or heap axis if so.
pub fn frontier_win(data: &Data, p: Point, e: &Entry, workload: &str, pool: &[&Entry]) -> Option<(String, f64)> {
    if !on_frontier(data, p, e, workload, pool) {
        return None;
    }
    // An encoder is weighed on encode axes too, and only against encoders; a
    // decode-only entry is weighed on decode axes against everyone.
    let encode = e.encodes;
    let mine = axes(data, p, e, workload, encode);
    let theirs = axes(data, p, &ENTRIES[0], workload, encode);
    let names: &[&str] = if encode {
        &["decode CPU", "decode heap", "encode CPU", "encode heap"]
    } else {
        &["decode CPU", "decode heap"]
    };
    names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let (m, t) = (mine[i], theirs[i]);
            let (Some(mv), Some(tv)) = (m.value, t.value) else {
                return None;
            };
            (tv > 0.0 && mv < tv * (1.0 - m.tol) && count_lower(&m, &t)).then(|| ((*name).to_owned(), mv / tv))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// A library's verdict.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub entry: Entry,
    pub rules: [Outcome; 4],
    pub recommended: Outcome,
    pub exists: Exists,
}

/// Why `e`'s cycles at `workload` cannot be weighed against `pool`: a cell
/// with no A/A control, or an entry timed there (`e`, serde_json, or any of
/// the pool it is weighed against) with no count at v3 to corroborate it.
/// An entry without a count can neither dominate nor be dominated, so one
/// missing anywhere in the pool leaves the frontier undecided.
fn unweighable(data: &Data, e: &Entry, workload: &str, pool: &[&Entry]) -> Vec<String> {
    let ops: &[&str] = if e.encodes { &["decode", "encode"] } else { &["decode"] };
    let mut gaps = Vec::new();
    for op in ops {
        if aa_band(data, X86_NATIVE, workload, op).is_none() {
            gaps.push(format!("no A/A control for {workload} {op}"));
            continue;
        }
        let others = pool.iter().filter(|x| *op == "decode" || x.encodes).map(|x| x.backend);
        for b in [e.backend, "serde_json"].into_iter().chain(others) {
            if cpu(data, X86_NATIVE, b, workload, op).is_some() && cpu(data, X86_V3, b, workload, op).is_none() {
                gaps.push(format!("no v3 count for {b} {workload} {op}"));
            }
        }
    }
    gaps.sort();
    gaps.dedup();
    gaps
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
    let ops: &[&str] = if e.encodes { &["decode", "encode"] } else { &["decode"] };
    let serde_json_counted = KYNOS.iter().all(|w| {
        ops.iter()
            .all(|op| cpu(data, X86_NATIVE, "serde_json", w, op).is_some())
    });
    let gaps: Vec<String> = KYNOS.iter().flat_map(|w| unweighable(data, e, w, pool)).collect();
    if !gaps.is_empty() || !serde_json_counted {
        return Outcome::Pending(format!(
            "cycles cannot be weighed or corroborated: {}",
            if gaps.is_empty() {
                "serde_json not timed at every Kynos cell".to_owned()
            } else {
                gaps.join(", ")
            }
        ));
    }
    let best = KYNOS
        .iter()
        .filter_map(|w| frontier_win(data, X86_NATIVE, e, w, pool).map(|(axis, r)| (*w, axis, r)))
        .min_by(|a, b| a.2.total_cmp(&b.2));
    match best {
        // Frontier membership rests on cycles even when the win is heap, so
        // the trust class of the cycles governs every win.
        Some((w, axis, r)) => {
            let win = format!("{w} {axis} {r:.2}× at {}", X86_NATIVE.win_label(&axis));
            match hw_trust(data) {
                "canonical" => Outcome::Pass(win),
                trust => Outcome::Pending(format!(
                    "provisional pass: {win} ({trust}); awaits a quiet-host rerun"
                )),
            }
        }
        // On `solo` timing a wide A/A band can hide a real win: that is no
        // failure until a quiet host has measured it.
        None if hw_trust(data) != "canonical"
            && KYNOS
                .iter()
                .any(|w| frontier_win(data, X86_NATIVE.without_band(), e, w, pool).is_some()) =>
        {
            Outcome::Pending(format!(
                "no win beyond the A/A band of {} timing, but one beyond ε alone; awaits a quiet-host rerun",
                hw_trust(data)
            ))
        }
        None => Outcome::Fail(
            "not on the frontier with a CPU or heap win beyond ε and the A/A band, corroborated at v3, on any Kynos shape"
                .into(),
        ),
    }
}

/// The trust class the hardware counters were taken under: `canonical` only
/// if every cell was.
pub fn hw_trust(data: &Data) -> &'static str {
    weakest(data.host.hw.rows.values())
}

/// `canonical` if every row says so, else `solo`.
pub fn weakest<'a>(mut rows: impl Iterator<Item = &'a Value>) -> &'static str {
    if rows.all(|r| r["trust"] == "canonical") {
        "canonical"
    } else {
        "solo"
    }
}

/// What a Miri row that found undefined behaviour reports, shortened:
/// "error: Undefined Behavior: <what>: <where>" -> "<what>".
fn miri_failure(r: &Value) -> Option<String> {
    if r["outcome"] != "undefined-behavior" {
        return None;
    }
    let what = r["detail"]
        .as_str()
        .and_then(|d| d.split("Undefined Behavior: ").nth(1))
        .and_then(|d| d.split(':').next())
        .map(str::trim)
        .filter(|w| !w.is_empty());
    Some(what.map_or_else(
        || "Miri reports undefined behavior".to_owned(),
        |w| format!("Miri reports undefined behavior ({w})"),
    ))
}

/// Each measured cell of `backend` whose result the cell's own check found
/// wrong: a conformance failure on the benchmark's own inputs.
fn wrong_results(data: &Data, backend: &str) -> Vec<String> {
    let mut wrong: Vec<String> = [&data.callgrind, &data.host.alloc, &data.host.hw, &data.host.time]
        .iter()
        .flat_map(|t| t.rows.iter())
        .filter(|(k, r)| k.backend == backend && r["verdict"] == "wrong")
        .map(|(k, _)| format!("wrong result on {} {} {} {}", k.workload, k.op, k.form, k.arrival))
        .collect();
    wrong.sort();
    wrong.dedup();
    wrong
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
    failures.extend(wrong_results(data, e.backend));
    let set = e.set;
    // Each backend is fuzzed on its own; its rows are its evidence alone.
    let fuzz: Vec<&Value> = data
        .fuzz
        .iter()
        .filter(|r| r["set"] == set && r["backend"] == e.backend)
        .collect();
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
    failures.extend(miri.and_then(miri_failure));
    // serde_json is what the suite compares against, so it has no
    // disagreements of its own; Miri still runs on it.
    if e.backend.starts_with("serde_json") {
        return if failures.is_empty() {
            Outcome::Pass("reference".into())
        } else {
            Outcome::Fail(failures.join("; "))
        };
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
    let expected = crate::fuzzing::configs(set).len() * crate::fuzzing::FUZZ_TARGETS.len();
    let finished = fuzz.iter().filter(|r| r["finished"] != false).count();
    if finished < expected {
        return Outcome::Pending(format!(
            "{runs} conformance runs agree; {finished} of {expected} fuzz runs finished"
        ));
    }
    match miri.and_then(|m| m["outcome"].as_str()) {
        None | Some("error") => Outcome::Pending(format!(
            "{runs} conformance runs and {finished} fuzz runs agree; Miri did not run"
        )),
        Some(outcome) => Outcome::Pass(format!(
            "{runs} conformance runs, {finished} fuzz runs, Miri {}",
            if outcome == "unsupported" {
                "cannot run this backend's intrinsics"
            } else {
                outcome
            }
        )),
    }
}

fn rule3(data: &Data, e: &Entry) -> Outcome {
    // A memory error the sanitizer caught while fuzzing is a soundness defect
    // whatever else is recorded, so it fails before a missing footprint.
    let mut problems: Vec<String> = data
        .fuzz
        .iter()
        .filter(|r| r["set"] == e.set && r["backend"] == e.backend && r["sanitizer_report"].is_string())
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

/// One route's end-to-end timing against serde_json's.
#[derive(Debug, Clone, Copy)]
pub struct Gain {
    /// Throughput change; positive is more requests per second.
    pub rps: f64,
    /// p99 latency change; negative is faster. `None` when either side could
    /// not keep up with the open-loop rate (saturated).
    pub p99: Option<f64>,
    /// serde_json's A/A copy against serde_json, as absolute fractions; `None`
    /// when the control is missing.
    pub aa_rps: Option<f64>,
    pub aa_p99: Option<f64>,
}

/// End-to-end timing of `backend` on `route` at `native`, against serde_json.
pub fn e2e_gain(rows: &[Value], backend: &str, route: &str) -> Option<Gain> {
    let find = |b: &str| {
        rows.iter()
            .find(|r| r["backend"] == b && r["route"] == route && r["variant"] == X86_NATIVE.variant)
    };
    let (me, sj) = (find(backend)?, find("serde_json")?);
    let aa = find("serde_json#aa");
    let sj_rps = sj["rps"].as_f64()?;
    let sj_p99 = sj["p99_ms"].as_f64();
    // The band is the widest gap between serde_json and its A/A copy, in the
    // medians or in any one rep (the two run in the same rep, so each rep's
    // pair is one sample of the noise).
    let band = |median: Option<f64>, run: &str, field: &str| -> Option<f64> {
        let reps = |r: &Value| -> Vec<Option<f64>> {
            r["reps"]
                .as_array()
                .map(|v| v.iter().map(|x| x[run][field].as_f64()).collect())
                .unwrap_or_default()
        };
        let a = aa?;
        let per_rep = reps(sj)
            .into_iter()
            .zip(reps(a))
            .filter_map(|(x, y)| Some((y? / x.filter(|x| *x > 0.0)? - 1.0).abs()))
            .fold(0.0, f64::max);
        Some(
            (a[if run == "closed" { "rps" } else { "p99_ms" }].as_f64()? / median? - 1.0)
                .abs()
                .max(per_rep),
        )
    };
    Some(Gain {
        rps: me["rps"].as_f64()? / sj_rps - 1.0,
        p99: me["p99_ms"].as_f64().zip(sj_p99).map(|(m, s)| m / s - 1.0),
        aa_rps: band(Some(sj_rps), "closed", "rps"),
        aa_p99: band(sj_p99, "open", "p99_ms"),
    })
}

/// A p99 change as text; a saturated run has none.
pub fn p99_text(p99: Option<f64>) -> String {
    p99.map_or("saturated".into(), |p| format!("{:+.0} %", p * 100.0))
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
    let rows = &data.host.e2e_time;
    let mut seen = false;
    let mut unweighable = Vec::new();
    let mut best: Option<String> = None;
    let mut inconsistent = Vec::new();
    for route in routes {
        let Some(g) = e2e_gain(rows, e.backend, route) else {
            continue;
        };
        seen = true;
        let predicted = e2e_predicted(data, e.backend, route);
        let (Some(aa_rps), Some(predicted)) = (g.aa_rps, predicted) else {
            unweighable.push((*route).to_owned());
            continue;
        };
        let pass_rps = g.rps >= E2E_GAIN && g.rps > aa_rps;
        // A saturated run has no p99 to weigh; it can still win on throughput.
        let pass_p99 = matches!((g.p99, g.aa_p99), (Some(p), Some(b)) if -p >= E2E_GAIN && -p > b);
        if !(pass_rps || pass_p99) {
            continue;
        }
        // The count bounds what the codec can give: more throughput than it
        // predicts is a measurement error, not a result. A p99 under load is
        // not proportional to the cost of a request (a cheaper request also
        // shortens every queue behind it), so it has no such bound; it counts
        // only where the count itself predicts a material gain.
        // Nor can a material predicted gain show as a throughput loss beyond
        // the noise: the count and the timing then disagree on direction.
        let slack = EPSILON.max(aa_rps);
        if g.rps > predicted + slack
            || (predicted > EPSILON && g.rps < -slack)
            || (pass_p99 && !pass_rps && predicted <= EPSILON)
        {
            inconsistent.push(format!(
                "{route}: {:+.0} % rps and p99 {} measured, {:+.0} % rps predicted",
                g.rps * 100.0,
                p99_text(g.p99),
                predicted * 100.0
            ));
            continue;
        }
        best.get_or_insert(format!("{route} {:+.0} % rps, p99 {}", g.rps * 100.0, p99_text(g.p99)));
    }
    let trust = weakest(rows.iter());
    match (seen, best) {
        (false, _) => Outcome::Pending("end-to-end timing not yet recorded".into()),
        (true, Some(b)) if trust == "canonical" => Outcome::Pass(b),
        (true, Some(b)) => Outcome::Pending(format!("provisional pass: {b} ({trust}); awaits a quiet-host rerun")),
        (true, None) if !unweighable.is_empty() => Outcome::Pending(format!(
            "no A/A control or counted prediction for {}",
            unweighable.join(", ")
        )),
        // More than the count allows is a measurement error, not a result:
        // the rule cannot be decided until a rerun measures it.
        (true, None) if !inconsistent.is_empty() => Outcome::Pending(format!(
            "gains inconsistent with counted codec share, so a measurement error; rerun: {}",
            inconsistent.join("; ")
        )),
        (true, None) => Outcome::Fail("no route ≥10 % better in throughput or p99 beyond the A/A band".into()),
    }
}

/// Whether `e` deserves to exist. Its frontier is drawn among `pool`: every
/// entry, eligible or not, that has not failed conformance. A win is read at
/// `native` hardware cycles (or heap) over every workload, gated as rule 1's
/// is (A/A band, counts pointing the same way, and a timed win marked
/// provisional unless taken on a quiet host), and a CPU win is stated with
/// its v3 count beside it.
fn exists(data: &Data, e: &Entry, pool: &[&Entry]) -> Exists {
    if e.backend == "serde_json" {
        return Exists::new(true, "the baseline".into(), false);
    }
    let solo = hw_trust(data) != "canonical";
    let mut best: Option<(f64, String)> = None;
    let mut unweighed = Vec::new();
    let mut hidden = false;
    for w in payloads::ALL {
        // Only workloads whose cycles can be weighed and corroborated count.
        if cpu(data, X86_NATIVE, e.backend, w, "decode").is_some() && !unweighable(data, e, w, pool).is_empty() {
            unweighed.push(*w);
            continue;
        }
        hidden |= solo && frontier_win(data, X86_NATIVE.without_band(), e, w, pool).is_some();
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
                format!(
                    "{w} {axis} {r:.2}× serde_json at {}{count}",
                    X86_NATIVE.win_label(&axis)
                ),
            ));
        }
    }
    let note = super::notes::merit(data, e.backend);
    let trust = hw_trust(data);
    let best = best.map(|(_, b)| if solo { format!("{b}, provisional ({trust})") } else { b });
    match (best, note) {
        (Some(b), Some(n)) => Exists::new(true, format!("{b}. {n}"), solo),
        (Some(b), None) => Exists::new(true, b, solo),
        // A merit stands on counts or conformance, not on timing.
        (None, Some(n)) => Exists::new(true, n, false),
        (None, None) if hidden || !unweighed.is_empty() => Exists::new(
            false,
            format!(
                "no measured reason yet{}{}",
                if hidden {
                    format!("; a win beyond ε alone is inside the A/A band of {trust} timing")
                } else {
                    String::new()
                },
                if unweighed.is_empty() {
                    String::new()
                } else {
                    format!("; cycles not weighable at {}", unweighed.join(", "))
                }
            ),
            true,
        ),
        (None, None) => Exists::new(false, "no measured reason to choose it over serde_json".into(), solo),
    }
}

/// Whether an entry deserves to exist, why, and whether that rests on
/// `solo` timing or undecided cells, so a quiet-host rerun could change it.
#[derive(Debug, Clone)]
pub struct Exists(pub bool, pub String, pub bool);

impl Exists {
    fn new(yes: bool, why: String, provisional: bool) -> Self {
        Self(yes, why, provisional)
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
            "exists_provisional": v.exists.2,
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

    /// Hardware cycles at native and callgrind counts at v3, the same at every
    /// Kynos shape and operation, per backend: `(backend, cycles,
    /// est_cycles)`.
    pub(in crate::report) fn measured(cells: &[(&str, f64, f64)], trust: &str) -> Data {
        let mut data = Data::default();
        data.host.slug = "x86_64-linux-amd-x".into();
        for (backend, cycles, est) in cells {
            for w in crate::matrix::KYNOS {
                for op in ["decode", "encode"] {
                    data.host.hw.rows.insert(
                        Key::new("native", backend, w, op),
                        json!({ "cycles": cycles, "trust": trust }),
                    );
                    data.callgrind
                        .rows
                        .insert(Key::new("v3", backend, w, op), json!({ "est_cycles": est }));
                }
            }
        }
        data
    }

    #[test]
    fn a_sanitizer_report_fails_rule3_even_without_a_footprint() {
        let e = sonic();
        let row = json!({ "set": e.set, "backend": e.backend, "config": "portable", "target": "decode_value",
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

    /// A cycles win whose count is a little higher (4 %, inside ε) is not
    /// borne out either: the count must point the same way.
    #[test]
    fn a_cycles_win_the_count_does_not_point_to_is_not_a_win() {
        let data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 101.0, 100.0),
                ("simd-json", 90.0, 104.0),
            ],
            "canonical",
        );
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == simd().backend).unwrap();
        assert!(matches!(simd.rules[0], Outcome::Fail(_)), "{:?}", simd.rules[0]);
    }

    /// Without counts at v3 the cycles cannot be corroborated: rule 1 waits,
    /// and nobody dominates on them.
    #[test]
    fn without_counts_rule1_is_pending() {
        let mut data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 101.0, 100.0),
                ("simd-json", 80.0, 90.0),
            ],
            "canonical",
        );
        data.callgrind.rows.clear();
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == simd().backend).unwrap();
        assert!(matches!(simd.rules[0], Outcome::Pending(_)), "{:?}", simd.rules[0]);
    }

    /// sonic-rs dominates simd-json, but has no count at v3: whether it
    /// does cannot be corroborated, so simd-json's rule 1 waits rather than
    /// passing for want of a challenger.
    #[test]
    fn a_challenger_without_counts_leaves_rule1_pending() {
        let mut data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 101.0, 100.0),
                ("simd-json", 80.0, 90.0),
                ("sonic-rs", 50.0, 50.0),
            ],
            "canonical",
        );
        data.callgrind.rows.retain(|k, _| k.backend != "sonic-rs");
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == simd().backend).unwrap();
        assert!(matches!(simd.rules[0], Outcome::Pending(_)), "{:?}", simd.rules[0]);
    }

    /// On `solo` timing, a win the counts confirm but a 40 % A/A band hides
    /// is not a failure: a quiet host decides it.
    #[test]
    fn a_win_only_solo_noise_hides_is_pending() {
        let data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 140.0, 100.0),
                ("simd-json", 70.0, 80.0),
            ],
            "solo",
        );
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == simd().backend).unwrap();
        assert!(matches!(simd.rules[0], Outcome::Pending(_)), "{:?}", simd.rules[0]);
        assert!(simd.exists.2, "{:?}", simd.exists);
    }

    /// A timed loss far beyond the band is a loss, whatever the count of
    /// another build says: twice the cycles at half the heap is a trade-off,
    /// not dominance.
    #[test]
    fn a_large_timed_loss_is_never_excused() {
        let a = [
            super::Axis {
                value: Some(200.0),
                tol: 0.05,
                count: super::Count::At(90.0),
            },
            super::Axis {
                value: Some(50.0),
                tol: 0.05,
                count: super::Count::Untimed,
            },
        ];
        let b = [
            super::Axis {
                value: Some(100.0),
                tol: 0.05,
                count: super::Count::At(100.0),
            },
            super::Axis {
                value: Some(100.0),
                tol: 0.05,
                count: super::Count::Untimed,
            },
        ];
        assert!(!super::dominates(&a, &b));
    }

    /// The A/A band is the widest gap in any round, not only the medians'.
    #[test]
    fn the_aa_band_covers_every_round() {
        let mut data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 100.0, 100.0),
                ("simd-json", 85.0, 90.0),
            ],
            "canonical",
        );
        for w in crate::matrix::KYNOS {
            for op in ["decode", "encode"] {
                for (b, rounds) in [
                    ("serde_json", [100.0, 100.0, 100.0]),
                    ("serde_json#aa", [100.0, 120.0, 100.0]),
                ] {
                    let row = data.host.hw.rows.get_mut(&Key::new("native", b, w, op)).unwrap();
                    row["rounds"] = json!(rounds.map(|c| json!({ "cycles": c })));
                }
            }
        }
        assert!((super::aa_band(&data, super::X86_NATIVE, "json-small", "decode").unwrap() - 0.2).abs() < 1e-9);
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == simd().backend).unwrap();
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

    /// sonic-rs measures fewer cycles than simd-json, but callgrind counts
    /// simd-json 30 % cheaper: the counts contradict sonic-rs's advantage, so
    /// it cannot push simd-json off the frontier.
    #[test]
    fn a_cycles_advantage_the_counts_contradict_dominates_nobody() {
        let data = measured(
            &[
                ("serde_json", 100.0, 100.0),
                ("serde_json#aa", 100.0, 100.0),
                ("sonic-rs", 60.0, 100.0),
                ("simd-json", 80.0, 70.0),
            ],
            "canonical",
        );
        let verdicts = super::judge(&data);
        let simd = verdicts.iter().find(|v| v.entry.backend == "simd-json").unwrap();
        assert_eq!(
            simd.rules[0],
            Outcome::Pass("json-small decode CPU 0.80× at native (cycles)".into())
        );
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
            "canonical",
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
        let row = json!({ "set": e.set, "backend": e.backend, "config": "v3", "target": "roundtrip",
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
            json!({ "set": e.set, "backend": e.backend, "config": config, "target": target,
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
