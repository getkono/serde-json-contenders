//! Loading `results/` into lookups.
//!
//! Loading is loud: a result file that is missing or does not parse is an
//! error, unless `results/skipped.toml` declares it skipped and says why, and
//! the report states that reason. An empty table never stands in for a
//! measurement nobody took.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::util::{read_json, root};

/// One measurement row's identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub variant: String,
    pub backend: String,
    pub workload: String,
    pub op: String,
    pub form: String,
    pub arrival: String,
}

impl Key {
    pub fn of(row: &Value) -> Self {
        let s = |k: &str| row[k].as_str().unwrap_or_default().to_owned();
        Self {
            variant: s("variant"),
            backend: s("backend"),
            workload: s("workload"),
            op: s("op"),
            form: s("form"),
            arrival: s("arrival"),
        }
    }

    pub fn new(variant: &str, backend: &str, workload: &str, op: &str) -> Self {
        Self {
            variant: variant.into(),
            backend: backend.into(),
            workload: workload.into(),
            op: op.into(),
            form: "owned".into(),
            arrival: "shared".into(),
        }
    }
}

/// Rows of one kind, by key.
#[derive(Debug, Default)]
pub struct Table {
    pub rows: BTreeMap<Key, Value>,
    pub provenance: Value,
}

impl Table {
    /// The rows of `doc`, keyed. Two rows with one key would leave one of
    /// them unread, so that is an error.
    fn of(doc: &Value) -> Result<Self> {
        let mut rows = BTreeMap::new();
        for r in doc["rows"].as_array().into_iter().flatten() {
            let key = Key::of(r);
            if rows.insert(key.clone(), r.clone()).is_some() {
                bail!("two rows for {key:?}");
            }
        }
        Ok(Self {
            rows,
            provenance: doc["provenance"].clone(),
        })
    }

    pub fn get(&self, key: &Key, field: &str) -> Option<f64> {
        self.rows.get(key).and_then(|r| r[field].as_f64())
    }

    pub fn row(&self, key: &Key) -> Option<&Value> {
        self.rows.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// One result file the report read, or was told to do without.
#[derive(Debug, Clone, Default)]
pub struct Source {
    /// Path relative to `results/`.
    pub name: String,
    /// The file's provenance stamp; null when skipped.
    pub provenance: Value,
    /// Why the file is absent, from `results/skipped.toml`.
    pub skipped: Option<String>,
}

/// Reads result files, recording each as a [`Source`].
struct Loader {
    results: std::path::PathBuf,
    skipped: toml::Table,
    sources: Vec<Source>,
    missing: Vec<String>,
}

impl Loader {
    /// The document at `name` (relative to `results/`), or `None` when it is
    /// declared skipped or missing (recorded either way).
    fn doc(&mut self, name: &str) -> Result<Option<Value>> {
        let path = self.results.join(name);
        if !path.exists() {
            match self
                .skipped
                .get(name)
                .and_then(|s| s.get("reason"))
                .and_then(toml::Value::as_str)
            {
                Some(reason) => self.sources.push(Source {
                    name: name.to_owned(),
                    provenance: Value::Null,
                    skipped: Some(reason.to_owned()),
                }),
                None => self.missing.push(name.to_owned()),
            }
            return Ok(None);
        }
        let doc = read_json(&path).with_context(|| format!("results/{name} does not parse"))?;
        self.sources.push(Source {
            name: name.to_owned(),
            provenance: doc["provenance"].clone(),
            skipped: None,
        });
        Ok(Some(doc))
    }

    fn table(&mut self, name: &str) -> Result<Table> {
        match self.doc(name)? {
            Some(d) => Table::of(&d).with_context(|| format!("results/{name}")),
            None => Ok(Table::default()),
        }
    }

    /// The rows of a per-set or per-route file. Two rows with one identity
    /// would leave one of them unread (lookups take the first), so that is
    /// an error.
    fn rows(&mut self, name: &str) -> Result<Vec<Value>> {
        self.stamped_rows(name).map(|(rows, _)| rows)
    }

    /// [`Self::rows`], with the file's provenance stamp. A file is read once:
    /// each read records a [`Source`].
    fn stamped_rows(&mut self, name: &str) -> Result<(Vec<Value>, Value)> {
        let doc = self.doc(name)?.unwrap_or(Value::Null);
        let rows = doc["rows"].as_array().cloned().unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        for r in &rows {
            let id: Vec<String> = ["variant", "set", "backend", "route", "config", "target"]
                .iter()
                .map(|k| r[*k].to_string())
                .collect();
            if !seen.insert(id.clone()) {
                bail!("results/{name}: two rows for {}", id.join(" "));
            }
        }
        Ok((rows, doc["provenance"].clone()))
    }
}

/// Every conformance run, at every variant and with `arbitrary_precision` at
/// portable, finished and reported every backend of its set: one that
/// errored, as a run or for one backend, left a SIMD path unchecked.
fn conformance_gaps(data: &Data) -> Vec<String> {
    let host = &data.host;
    let mut gaps = Vec::new();
    for variant in crate::matrix::variants() {
        for set in crate::matrix::SETS {
            let mut names = vec![set.name.to_owned()];
            if variant.name == "portable" {
                names.push(format!("{}+arbitrary-precision", set.name));
            }
            for name in names {
                let file = format!("conformance/{}/{name}.json", variant.name);
                let Some(report) = host.conformance.get(&(variant.name.to_owned(), name.clone())) else {
                    continue; // missing or skipped: the loader has said so
                };
                let run_error = report["error"].as_str();
                if let Some(e) = run_error {
                    gaps.push(format!("{file}: the suite did not finish ({e})"));
                }
                for b in set.backends.iter().filter(|b| !b.starts_with("serde_json")) {
                    let label = crate::matrix::label(set, b);
                    let backend = &report["backends"][label.as_str()];
                    if let Some(e) = backend["error"].as_str() {
                        gaps.push(format!("{file}: the {label} suite did not finish ({e})"));
                    } else if backend.is_null() && run_error.is_none() {
                        gaps.push(format!("{file}: no result for {label}"));
                    }
                }
            }
        }
    }
    gaps
}

/// Every file under `dir`, as paths relative to it.
fn files_under(dir: &std::path::Path, prefix: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            out.extend(files_under(&entry.path(), &format!("{name}/"))?);
        } else {
            out.push(name);
        }
    }
    Ok(out)
}

/// The host's results.
#[derive(Debug, Default)]
pub struct Host {
    pub slug: String,
    pub alloc: Table,
    pub hw: Table,
    pub time: Table,
    pub size: Vec<Value>,
    pub compile: Vec<Value>,
    pub msrv: Vec<Value>,
    pub e2e_time: Vec<Value>,
    pub e2e_time_provenance: Value,
    pub doctor: Value,
    /// conformance reports: (variant, set) -> report.
    pub conformance: BTreeMap<(String, String), Value>,
    /// The host's CPU, from its allocation counts' stamp.
    pub provenance: Value,
}

/// Everything under `results/`.
#[derive(Debug, Default)]
pub struct Data {
    /// callgrind counts per operation.
    pub callgrind: Table,
    /// callgrind counts per HTTP request.
    pub e2e_count: Vec<Value>,
    /// The zero-tolerance recount of every committed count.
    pub recount: Value,
    pub host: Host,
    pub footprint: Vec<Value>,
    pub advisory_db: String,
    pub fuzz: Vec<Value>,
    pub miri: Vec<Value>,
    pub soundness: toml::Table,
    pub notes: toml::Table,
    /// Every result file read or declared skipped, in load order.
    pub sources: Vec<Source>,
}

impl Data {
    /// Load `results/`: the one host directory (this benchmark measures one
    /// x86-64 Linux host), the callgrind counts, and the host-independent
    /// files.
    pub fn load() -> Result<Self> {
        let results = root().join("results");
        let toml = |name: &str| -> Result<toml::Table> {
            let path = results.join(name);
            if !path.exists() {
                return Ok(toml::Table::new());
            }
            std::fs::read_to_string(&path)?
                .parse()
                .with_context(|| format!("results/{name} does not parse"))
        };
        let mut load = Loader {
            results: results.clone(),
            skipped: toml("skipped.toml")?,
            sources: Vec::new(),
            missing: Vec::new(),
        };
        let mut data = Self {
            callgrind: load.table("callgrind/x86_64.json")?,
            e2e_count: load.rows("callgrind/e2e-x86_64.json")?,
            recount: load.doc("callgrind/recount.json")?.unwrap_or(Value::Null),
            ..Self::default()
        };
        let hosts: Vec<String> = std::fs::read_dir(&results)?
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir() && e.file_name() != "callgrind")
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let [slug] = hosts.as_slice() else {
            bail!("results/ must hold exactly one host directory, found {hosts:?}");
        };
        if !slug.starts_with("x86_64-linux-") {
            bail!("results/{slug}: this benchmark measures x86-64 Linux only");
        }
        data.host = load_host(&mut load, slug)?;
        let footprint = load.doc("footprint.json")?.unwrap_or(Value::Null);
        data.footprint = footprint["rows"].as_array().cloned().unwrap_or_default();
        footprint["advisory_db"]
            .as_str()
            .unwrap_or_default()
            .clone_into(&mut data.advisory_db);
        data.fuzz = load.rows("fuzz.json")?;
        data.miri = load.rows("miri.json")?;
        data.soundness = toml("soundness.toml")?;
        data.notes = toml("notes.toml")?;
        if !load.missing.is_empty() {
            bail!(
                "missing result files (measure them, or declare each skipped with a reason in results/skipped.toml): {}",
                load.missing.join(", ")
            );
        }
        // Every file under results/ is read: one no table reads would be
        // neither checked for staleness nor published.
        let read: std::collections::BTreeSet<String> = load
            .sources
            .iter()
            .map(|s| s.name.clone())
            .chain(["skipped.toml", "soundness.toml", "notes.toml"].map(str::to_owned))
            .collect();
        let unread: Vec<String> = files_under(&results, "")?
            .into_iter()
            .filter(|f| !f.starts_with("raw/") && !read.contains(f))
            .collect();
        if !unread.is_empty() {
            bail!(
                "results/ holds files the report does not read (delete them, or teach the report): {}",
                unread.join(", ")
            );
        }
        data.sources = load.sources;
        Ok(data)
    }

    /// The host whose results the tables quote.
    pub fn primary(&self) -> Option<&Host> {
        (!self.host.slug.is_empty()).then_some(&self.host)
    }
}

/// The name suffix of a run with serde_json's `arbitrary_precision` unified in.
const ARBITRARY_PRECISION: &str = "+arbitrary-precision";

/// A case of a section: (section, case).
type Case = (String, String);

/// A recorded reference outcome as the suite's `agree` tells outcomes apart:
/// accept, reject at a status, or panic. `None` for a reference that is not an
/// outcome (a rendered float, a limit). Two accepted values are the same
/// outcome here: how a number is held is the suite's own question to settle.
fn outcome_kind(reference: &Value) -> Option<String> {
    let outcome = reference.get("outcome").unwrap_or(reference);
    if outcome.get("accept").is_some() {
        Some("accept".to_owned())
    } else if outcome.get("panic").is_some() {
        Some("panic".to_owned())
    } else {
        outcome.get("reject").map(|r| format!("reject {}", r["status"]))
    }
}

/// Every backend entry of a report.
fn entries(report: &Value) -> impl Iterator<Item = &Value> {
    report["backends"].as_object().into_iter().flat_map(|b| b.values())
}

/// Every backend entry of a report that holds a result: a harness failure is
/// no evidence for or against a case.
fn measured(report: &Value) -> impl Iterator<Item = &Value> {
    entries(report).filter(|b| !b["error"].is_string())
}

/// Whether a harness failure, of the run or of one backend, cut a report short.
fn harness_failed(report: &Value) -> bool {
    report["error"].is_string() || entries(report).any(|b| b["error"].is_string())
}

/// serde_json's recorded outcome for every case any backend disagreed with it
/// on, in `reports`. `None` where the record is not an outcome.
fn recorded_references<'a>(reports: impl Iterator<Item = &'a Value>) -> BTreeMap<Case, Option<String>> {
    let mut recorded = BTreeMap::new();
    for (section, s) in reports
        .flat_map(entries)
        .flat_map(|b| b["sections"].as_object().into_iter().flatten())
    {
        for d in s["differences"].as_array().into_iter().flatten() {
            if let Some(case) = d["case"].as_str() {
                recorded
                    .entry((section.clone(), case.to_owned()))
                    .or_insert_with(|| outcome_kind(&d["reference"]));
            }
        }
    }
    recorded
}

/// The cases a backend entry lists as gating.
fn listed_failures(backend: &Value) -> impl Iterator<Item = Case> + '_ {
    backend["gating_failures"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f["section"].as_str()?.to_owned(), f["case"].as_str()?.to_owned())))
}

/// The cases where `arbitrary_precision` changes serde_json's own outcome.
/// The suite compares each backend with serde_json in the same build and
/// records no outcome of serde_json's on its own, so the evidence is what the
/// reports hold, and a case is taken only on one of two proofs:
///
/// 1. Its reference outcome is recorded in both builds, and they differ.
/// 2. No outcome is recorded by default, because no backend of any set failed
///    the case there (in sections whose failure lists are complete), and every
///    backend of every set fails it with `arbitrary_precision`, with at least
///    two sets measuring a backend. Backends that share nothing but serde_json
///    do not all change together, and one backend still passing is evidence
///    against the move, so that case stays gated.
///
/// A harness failure is a gap the report already refuses, not evidence: an
/// errored backend or run is left out of both proofs rather than withdrawing
/// them, so it cannot turn into a failed gate on another backend. The two sets
/// the second proof needs must still have measured something.
fn reference_moves(conformance: &BTreeMap<(String, String), Value>) -> std::collections::BTreeSet<Case> {
    let portable = |name: &str| conformance.get(&("portable".to_owned(), name.to_owned()));
    let has_backends = |report: &Value| measured(report).next().is_some();
    // Each measured build with `arbitrary_precision`, beside its default run.
    let mut pairs = Vec::new();
    let mut paired = true;
    for ((variant, name), report) in conformance {
        let Some(set) = name.strip_suffix(ARBITRARY_PRECISION).filter(|_| variant == "portable") else {
            continue;
        };
        match portable(set) {
            Some(default) if has_backends(report) && has_backends(default) => pairs.push((report, default)),
            // A serde_json-only set measures no backend in either build.
            Some(default) if !has_backends(report) && !has_backends(default) => {}
            // One build lost every backend to the harness: this set says nothing.
            Some(default) if harness_failed(report) || harness_failed(default) => {}
            _ => paired = false,
        }
    }
    let by_default = recorded_references(pairs.iter().map(|(_, d)| *d));
    let with_ap = recorded_references(pairs.iter().map(|(a, _)| *a));
    let mut moves: std::collections::BTreeSet<Case> = with_ap
        .iter()
        .filter(|(case, kind)| kind.is_some() && matches!(by_default.get(*case), Some(d) if d.is_some() && d != *kind))
        .map(|(case, _)| case.clone())
        .collect();
    if !paired || pairs.len() < 2 {
        return moves;
    }
    // A section's list is complete when it holds as many as were counted.
    let complete = |backend: &Value, section: &str| {
        let listed = listed_failures(backend).filter(|(s, _)| s == section).count() as u64;
        backend["sections"][section]["gating_failure_count"].as_u64() == Some(listed)
    };
    let mut everywhere: Option<std::collections::BTreeSet<Case>> = None;
    for backend in pairs.iter().flat_map(|(a, _)| measured(a)) {
        let cases = listed_failures(backend).collect();
        everywhere = Some(match everywhere {
            Some(common) => common.intersection(&cases).cloned().collect(),
            None => cases,
        });
    }
    moves.extend(everywhere.into_iter().flatten().filter(|case| {
        !by_default.contains_key(case)
            && pairs
                .iter()
                .flat_map(|(_, d)| measured(d))
                .all(|backend| complete(backend, &case.0) && listed_failures(backend).all(|listed| listed != *case))
    }));
    moves
}

/// Whether this backend panicked or crashed on the case: either is the
/// backend's defect whatever serde_json answers. The suite words a panic as
/// `panicked: `, `encode panicked: `, or inside a `decoded {"error":...}`
/// detail, and a depth probe's death as `crashed: `.
fn panicked(backend: &Value, section: &str, case: &str, detail: &Value) -> bool {
    detail
        .as_str()
        .is_some_and(|d| d.contains("panicked") || d.starts_with("crashed:"))
        || backend["sections"][section]["differences"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|d| d["case"] == case && d["backend"].get("panic").is_some())
}

/// Drop from one backend's entry the failures on `moves`, and restate its
/// counts and gate from what is left. Its `differences` keep the disagreement.
fn excuse(backend: &mut Value, moves: &std::collections::BTreeSet<Case>) {
    let Some(failures) = backend["gating_failures"].as_array() else {
        return;
    };
    let (excused, kept): (Vec<&Value>, Vec<&Value>) = failures.iter().partition(|f| {
        let (Some(section), Some(case)) = (f["section"].as_str(), f["case"].as_str()) else {
            return false;
        };
        moves.contains(&(section.to_owned(), case.to_owned())) && !panicked(backend, section, case, &f["detail"])
    });
    let sections: Vec<String> = excused
        .iter()
        .filter_map(|f| f["section"].as_str().map(str::to_owned))
        .collect();
    let kept: Vec<Value> = kept.into_iter().cloned().collect();
    for section in sections {
        let s = &mut backend["sections"][section.as_str()];
        if let Some(count) = s["gating_failure_count"].as_u64() {
            let count = count.saturating_sub(1);
            s["gating_failure_count"] = count.into();
            s["gating"] = (count > 0).into();
        }
    }
    // Failures past a section's listing cap are still failures.
    let unlisted = backend["sections"]
        .as_object()
        .is_some_and(|s| s.values().any(|s| s["gating_failure_count"].as_u64().unwrap_or(0) > 0));
    if kept.is_empty() && !unlisted {
        backend["gate"] = "pass".into();
    }
    backend["gating_failures"] = kept.into();
}

/// A backend matching serde_json is not at fault where `arbitrary_precision`
/// changes serde_json's own answer, so those cases do not gate in that build.
/// Applied as the results load, so the verdict and every report table read one
/// account of what gates; the default runs are never changed.
pub(crate) fn excuse_reference_moves(conformance: &mut BTreeMap<(String, String), Value>) {
    let moves = reference_moves(conformance);
    for ((variant, name), report) in conformance.iter_mut() {
        if variant != "portable" || !name.ends_with(ARBITRARY_PRECISION) {
            continue;
        }
        let Some(backends) = report["backends"].as_object_mut() else {
            continue;
        };
        for backend in backends.values_mut() {
            excuse(backend, &moves);
        }
    }
}

fn load_host(load: &mut Loader, slug: &str) -> Result<Host> {
    let at = |file: &str| format!("{slug}/{file}");
    let (e2e_time, e2e_time_provenance) = load.stamped_rows(&at("e2e-time.json"))?;
    let mut conformance = BTreeMap::new();
    for variant in crate::matrix::variants() {
        for set in crate::matrix::SETS {
            let mut names = vec![set.name.to_owned()];
            if variant.name == "portable" {
                names.push(format!("{}{ARBITRARY_PRECISION}", set.name));
            }
            for name in names {
                if let Some(doc) = load.doc(&at(&format!("conformance/{}/{name}.json", variant.name)))? {
                    conformance.insert((variant.name.to_owned(), name), doc["report"].clone());
                }
            }
        }
    }
    excuse_reference_moves(&mut conformance);
    let alloc = load.table(&at("alloc.json"))?;
    let provenance = alloc.provenance.clone();
    Ok(Host {
        slug: slug.to_owned(),
        hw: load.table(&at("hw.json"))?,
        time: load.table(&at("time.json"))?,
        alloc,
        size: load.rows(&at("size.json"))?,
        compile: load.rows(&at("compile-time.json"))?,
        msrv: load.rows(&at("msrv.json"))?,
        e2e_time,
        e2e_time_provenance,
        doctor: load.doc(&at("doctor.json"))?.unwrap_or(Value::Null),
        conformance,
        provenance,
    })
}

/// Why each source cannot be trusted for the code at HEAD: measured on other
/// code (another fingerprint), from a dirty checkout, or on a host other than
/// `host`, the results' host directory (so every source shares one host).
/// Empty when every source was measured on exactly the committed code, on
/// that host.
#[must_use]
pub fn stale(sources: &[Source], fingerprint: Option<&str>, host: &str) -> Vec<String> {
    sources
        .iter()
        .filter(|s| s.skipped.is_none())
        .filter_map(|s| {
            let p = &s.provenance;
            let theirs = p["fingerprint"].as_str();
            if p["git_dirty"] != false {
                Some(format!("{}: measured from a dirty checkout", s.name))
            } else if theirs.is_none() || theirs != fingerprint {
                Some(format!(
                    "{}: measured on code {} (HEAD is {})",
                    s.name,
                    theirs.unwrap_or("without a fingerprint"),
                    fingerprint.unwrap_or("unknown")
                ))
            } else if p["host"].as_str() != Some(host) {
                Some(format!(
                    "{}: measured on host {} (results are for {host})",
                    s.name,
                    p["host"].as_str().unwrap_or("without a host stamp"),
                ))
            } else {
                None
            }
        })
        .collect()
}

/// One per-operation table to check: its name, rows, variants, workloads,
/// shapes per workload, and whether serde_json has an A/A copy.
type PerOperation<'a> = (
    &'a str,
    &'a Table,
    &'a [&'a str],
    &'a [String],
    &'a dyn Fn(&str) -> &'static [crate::matrix::Shape],
    bool,
);

/// What a complete run of `mise run all` would have written and did not:
/// every row of the matrix in each per-operation table (a row recording the
/// backend's own rejection is present, and so is a wrong result, which rule
/// 2 reads; a row recording an error is a failure of the run), every set in the cost tables, every
/// server and route end to end, every fuzz target and configuration, Miri
/// for every set, a full zero-tolerance recount, and a doctor run without
/// failures.
#[must_use]
pub fn incomplete(data: &Data) -> Vec<String> {
    use crate::matrix::{COUNTED_VARIANTS, SHAPES, TIMED_SHAPES, hw_shapes};
    let mut gaps = Vec::new();
    let all: Vec<String> = payloads::ALL.iter().map(|w| (*w).to_owned()).collect();
    let host = &data.host;
    let tables: [PerOperation<'_>; 4] = [
        (
            "callgrind/x86_64.json",
            &data.callgrind,
            COUNTED_VARIANTS,
            &all,
            &|_| SHAPES,
            false,
        ),
        ("alloc.json", &host.alloc, &["native"], &all, &|_| SHAPES, false),
        ("hw.json", &host.hw, &["native"], &all, &hw_shapes, true),
        (
            "time.json",
            &host.time,
            &["native"],
            &crate::timed::default_workloads(),
            &|_| TIMED_SHAPES,
            true,
        ),
    ];
    for (name, table, variants, workloads, shapes, aa) in tables {
        gaps.extend(per_operation(name, table, variants, workloads, shapes, aa));
    }
    gaps.extend(per_set(data));
    gaps.extend(fuzz_and_miri(data));
    if !skipped(data, "callgrind/e2e-x86_64.json") {
        gaps.extend(end_to_end(
            &data.e2e_count,
            "callgrind/e2e-x86_64.json",
            COUNTED_VARIANTS,
            false,
        ));
    }
    if !skipped(data, &format!("{}/e2e-time.json", host.slug)) {
        gaps.extend(end_to_end(&host.e2e_time, "e2e-time.json", &["native"], true));
    }
    let r = &data.recount;
    let counted = data.callgrind.rows.values().filter(|r| r["ir"].is_number()).count();
    if !r.is_null()
        && (r["tolerance"] != 0.0 || r["compared"] != r["committed"] || r["committed"] != counted || counted == 0)
    {
        gaps.push(format!(
            "callgrind/recount.json: {} of {} counts recounted at tolerance {}, against {counted} counts committed now; the check needs every one of them at 0",
            r["compared"], r["committed"], r["tolerance"]
        ));
    }
    let builds = host.doctor["builds"].as_array().map_or(0, Vec::len);
    let expected = crate::matrix::variants().len() * crate::matrix::SETS.len();
    if !host.doctor.is_null() && builds != expected {
        gaps.push(format!(
            "doctor.json: {builds} builds checked, not every set at every variant ({expected})"
        ));
    }
    if let Some(f) = host.doctor["failures"].as_array().filter(|f| !f.is_empty()) {
        gaps.push(format!(
            "doctor.json: {} builds do not run the path they claim (first: {})",
            f.len(),
            f[0]
        ));
    }
    gaps.extend(conformance_gaps(data));
    // Rule 3 reads the hand review of open issues; a crate missing from it
    // would pass unreviewed.
    let mut unreviewed: Vec<&str> = super::verdict::ENTRIES
        .iter()
        .map(|e| e.krate)
        .filter(|k| {
            data.soundness
                .get(*k)
                .and_then(|t| t.get("open"))
                .and_then(toml::Value::as_array)
                .is_none()
        })
        .collect();
    unreviewed.dedup();
    if !unreviewed.is_empty() {
        gaps.push(format!("soundness.toml: no `open` list for {}", unreviewed.join(", ")));
    }
    gaps
}

/// Whether `results/skipped.toml` declared `name` skipped (and it is absent).
fn skipped(data: &Data, name: &str) -> bool {
    data.sources.iter().any(|s| s.name == name && s.skipped.is_some())
}

/// The rows of one per-operation table that are missing or failed.
fn per_operation(
    name: &str,
    table: &Table,
    variants: &[&str],
    workloads: &[String],
    shapes: &dyn Fn(&str) -> &'static [crate::matrix::Shape],
    aa: bool,
) -> Option<String> {
    use crate::matrix::{SETS, label};
    if table.provenance.is_null() {
        return None;
    }
    let (mut missing, mut failed) = (0usize, 0usize);
    let mut first = None;
    for variant in variants {
        for set in SETS {
            for backend in set.backends {
                let mut labels = vec![label(set, backend)];
                if aa && set.name == "baseline" {
                    labels.push(format!("{backend}#aa"));
                }
                for b in &labels {
                    for w in workloads {
                        for s in crate::matrix::for_workload(w, shapes(w)) {
                            let key = Key {
                                variant: (*variant).to_owned(),
                                backend: b.clone(),
                                workload: w.clone(),
                                op: s.op.to_owned(),
                                form: s.form.to_owned(),
                                arrival: s.arrival.to_owned(),
                            };
                            let what = match table.row(&key) {
                                None => "missing",
                                // A wrong result is a result (rule 2 reads it);
                                // an error is a cell that did not run.
                                Some(r) if r["error"].is_string() => "failed",
                                Some(_) => continue,
                            };
                            if what == "missing" {
                                missing += 1;
                            } else {
                                failed += 1;
                            }
                            first.get_or_insert(format!("{what}: {variant} {b} {w} {} {} {}", s.op, s.form, s.arrival));
                        }
                    }
                }
            }
        }
    }
    (missing + failed > 0).then(|| {
        format!(
            "{name}: {missing} rows missing, {failed} failed (first {})",
            first.unwrap_or_default()
        )
    })
}

/// The sets the per-set tables have no row for.
fn per_set(data: &Data) -> Vec<String> {
    use crate::matrix::SETS;
    let host = &data.host;
    let mut gaps = Vec::new();
    let at = |file: &str| format!("{}/{file}", host.slug);
    for (name, rows, per_variant) in [
        (at("size.json"), &host.size, true),
        (at("compile-time.json"), &host.compile, false),
        (at("msrv.json"), &host.msrv, false),
        ("footprint.json".to_owned(), &data.footprint, false),
        ("miri.json".to_owned(), &data.miri, false),
    ] {
        let variants: Vec<&str> = if per_variant {
            crate::matrix::variants().iter().map(|v| v.name).collect()
        } else {
            vec![""]
        };
        let lacking: Vec<String> = SETS
            .iter()
            .flat_map(|s| variants.iter().map(move |v| (s.name, *v)))
            .filter(|(set, v)| {
                !rows
                    .iter()
                    .any(|r| r["set"] == *set && (v.is_empty() || r["variant"] == *v))
            })
            .map(|(set, v)| format!("{set} {v}").trim().to_owned())
            .collect();
        if !lacking.is_empty() && !skipped(data, &name) {
            gaps.push(format!("{name}: no row for {}", lacking.join(", ")));
        }
    }
    gaps
}

/// Fuzz runs that are missing or did not finish, and Miri runs that failed
/// to run at all.
fn fuzz_and_miri(data: &Data) -> Vec<String> {
    use crate::matrix::SETS;
    let mut gaps = Vec::new();
    let lacking: Vec<String> = SETS
        .iter()
        .flat_map(crate::fuzzing::runs)
        .filter(|(set, backend, c, _, t)| {
            !data
                .fuzz
                .iter()
                .any(|r| r["set"] == set.name && r["backend"] == *backend && r["config"] == *c && r["target"] == *t)
        })
        .map(|(_, backend, c, _, t)| format!("{backend} {c} {t}"))
        .collect();
    if !lacking.is_empty() && !skipped(data, "fuzz.json") {
        gaps.push(format!("fuzz.json: no run for {}", lacking.join(", ")));
    }
    let unfinished: Vec<String> = data
        .fuzz
        .iter()
        .filter(|r| r["finished"] == false)
        .map(|r| format!("{} {} {}", r["set"], r["config"], r["target"]))
        .collect();
    if !unfinished.is_empty() {
        gaps.push(format!(
            "fuzz.json: runs that did not finish: {}",
            unfinished.join(", ")
        ));
    }
    let errored: Vec<String> = data
        .miri
        .iter()
        .filter(|r| r["outcome"] == "error")
        .map(|r| r["set"].to_string())
        .collect();
    if !errored.is_empty() {
        gaps.push(format!("miri.json: Miri itself failed for {}", errored.join(", ")));
    }
    gaps
}

/// The servers and routes an end-to-end file lacks, or whose run failed.
fn end_to_end(rows: &[Value], name: &str, variants: &[&str], aa: bool) -> Option<String> {
    use crate::matrix::{SETS, label};
    let mut lacking = Vec::new();
    for v in variants {
        for set in SETS {
            for backend in crate::endtoend::backends(set) {
                // The floor is counted, not timed.
                if aa && backend == "floor" {
                    continue;
                }
                let mut labels = vec![if backend == "floor" {
                    "floor".to_owned()
                } else {
                    label(set, backend)
                }];
                if aa && set.name == "baseline" {
                    labels.push(format!("{backend}#aa"));
                }
                for b in labels {
                    for route in crate::endtoend::routes_for(backend) {
                        let row = rows
                            .iter()
                            .find(|r| r["variant"] == *v && r["backend"] == b.as_str() && r["route"] == route.0);
                        // A wrong answer is a result, which rule 2 reads.
                        if row.is_none_or(|r| r["error"].is_string() && r["verdict"] != "wrong") {
                            lacking.push(format!("{v} {b} {}", route.0));
                        }
                    }
                }
            }
        }
    }
    (!lacking.is_empty()).then(|| {
        format!(
            "{name}: {} rows missing or failed (first {})",
            lacking.len(),
            lacking[0]
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::{Source, excuse_reference_moves, stale};

    const HOST: &str = "x86_64-linux-cpu";

    /// A backend entry failing `cases` in `section`, each recorded as a
    /// difference with `reference` and `outcome`.
    fn failing(section: &str, cases: &[&str], reference: &Value, outcome: &Value) -> Value {
        let failures: Vec<Value> = cases
            .iter()
            .map(|c| json!({ "section": section, "case": c, "detail": "outcome differs from serde_json" }))
            .collect();
        let differences: Vec<Value> = cases
            .iter()
            .map(|c| json!({ "case": c, "reference": reference, "backend": outcome }))
            .collect();
        json!({
            "gate": if cases.is_empty() { "pass" } else { "fail" },
            "gating_failures": failures,
            "sections": { section: {
                "gating": !cases.is_empty(),
                "gating_failure_count": cases.len(),
                "differences": differences,
            } },
        })
    }

    fn accept() -> Value {
        json!({ "accept": 1 })
    }

    fn reject() -> Value {
        json!({ "reject": { "status": 400 } })
    }

    /// Portable reports for each (set, default entry, `arbitrary_precision` entry).
    fn reports(sets: Vec<(&str, Value, Value)>) -> BTreeMap<(String, String), Value> {
        let mut reports = BTreeMap::new();
        for (set, default, ap) in sets {
            reports.insert(
                ("portable".to_owned(), set.to_owned()),
                json!({ "backends": { set: default } }),
            );
            reports.insert(
                ("portable".to_owned(), format!("{set}+arbitrary-precision")),
                json!({ "backends": { set: ap } }),
            );
        }
        reports
    }

    fn ap_entry<'a>(reports: &'a BTreeMap<(String, String), Value>, set: &str) -> &'a Value {
        &reports[&("portable".to_owned(), format!("{set}+arbitrary-precision"))]["backends"][set]
    }

    /// serde_json refuses by default and accepts with `arbitrary_precision`,
    /// and both builds' outcomes are on record: the backend matched the
    /// default and is excused in the other build only.
    #[test]
    fn a_recorded_change_of_serde_jsons_outcome_is_excused() {
        let mut reports = reports(vec![
            (
                "a",
                failing("numbers", &[], &reject(), &accept()),
                failing("numbers", &["1e400#value"], &accept(), &reject()),
            ),
            (
                "b",
                failing("numbers", &["1e400#value"], &reject(), &accept()),
                failing("numbers", &[], &accept(), &accept()),
            ),
        ]);
        excuse_reference_moves(&mut reports);
        let a = ap_entry(&reports, "a");
        assert_eq!(a["gate"], "pass");
        assert_eq!(a["gating_failures"], json!([]));
        assert_eq!(a["sections"]["numbers"]["gating_failure_count"], 0);
        assert_eq!(a["sections"]["numbers"]["gating"], false);
        // The disagreement stays on record.
        assert_eq!(a["sections"]["numbers"]["differences"][0]["case"], "1e400#value");
        // The default run gates what it gated.
        let b = &reports[&("portable".to_owned(), "b".to_owned())]["backends"]["b"];
        assert_eq!(b["gate"], "fail");
    }

    /// serde_json gave the same outcome in both builds, so the failure is the
    /// backend's own, even though it fails only with `arbitrary_precision`.
    #[test]
    fn a_failure_with_the_same_reference_in_both_builds_stays() {
        let mut reports = reports(vec![
            (
                "a",
                failing("numbers", &[], &accept(), &accept()),
                failing("numbers", &["x#value"], &accept(), &reject()),
            ),
            (
                "b",
                failing("numbers", &["x#value"], &accept(), &reject()),
                failing("numbers", &[], &accept(), &accept()),
            ),
        ]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// The same case, with two different accepted values: how a number is held
    /// is not a change of outcome.
    #[test]
    fn two_accepted_values_are_the_same_outcome() {
        let mut reports = reports(vec![
            (
                "a",
                failing("numbers", &[], &json!({ "accept": 1.0 }), &accept()),
                failing("numbers", &["x#value"], &json!({ "accept": "1.0" }), &reject()),
            ),
            (
                "b",
                failing("numbers", &["x#value"], &json!({ "accept": 1.0 }), &reject()),
                failing("numbers", &[], &accept(), &accept()),
            ),
        ]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// No backend failed by default, so serde_json's default outcome is not
    /// on record: every backend of every set failing only with
    /// `arbitrary_precision` is the proof.
    #[test]
    fn every_set_failing_only_with_arbitrary_precision_is_excused() {
        let flatten = |r: Value| {
            failing(
                "attributes",
                &["flatten/0"],
                &json!({ "input": "{}", "outcome": r }),
                &accept(),
            )
        };
        let clean = || failing("attributes", &[], &accept(), &accept());
        let mut reports = reports(vec![
            ("a", clean(), flatten(reject())),
            ("b", clean(), flatten(reject())),
        ]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "pass");
        assert_eq!(ap_entry(&reports, "b")["gate"], "pass");
    }

    /// A set measuring no backend, as `baseline` does in both builds, says
    /// nothing about a case and does not stop the others proving it.
    #[test]
    fn a_serde_json_only_set_does_not_block_the_proof() {
        let flatten = || failing("attributes", &["flatten/0"], &reject(), &accept());
        let clean = || failing("attributes", &[], &accept(), &accept());
        let mut reports = reports(vec![("a", clean(), flatten()), ("b", clean(), flatten())]);
        for name in ["baseline", "baseline+arbitrary-precision"] {
            reports.insert(("portable".to_owned(), name.to_owned()), json!({ "backends": {} }));
        }
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "pass");
        assert_eq!(ap_entry(&reports, "b")["gate"], "pass");
    }

    /// Two sets failing beside a third that passes do not prove a move: the
    /// third answers as serde_json does in that build.
    #[test]
    fn a_passing_set_keeps_the_others_gated() {
        let flatten = || failing("attributes", &["flatten/0"], &reject(), &accept());
        let clean = || failing("attributes", &[], &accept(), &accept());
        let mut reports = reports(vec![
            ("a", clean(), flatten()),
            ("b", clean(), flatten()),
            ("c", clean(), clean()),
        ]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
        assert_eq!(ap_entry(&reports, "b")["gate"], "fail");
    }

    /// One library failing alone is that library's change, not serde_json's.
    #[test]
    fn one_set_failing_alone_stays() {
        let clean = || failing("attributes", &[], &accept(), &accept());
        let mut reports = reports(vec![
            (
                "a",
                clean(),
                failing("attributes", &["c/0"], &json!({ "outcome": reject() }), &accept()),
            ),
            ("b", clean(), clean()),
        ]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// A single set has no other backend to compare with.
    #[test]
    fn a_lone_set_proves_nothing() {
        let mut reports = reports(vec![(
            "a",
            failing("attributes", &[], &accept(), &accept()),
            failing("attributes", &["c/0"], &json!({ "outcome": reject() }), &accept()),
        )]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// A default run whose failure list was capped might hold the case past
    /// the cap, so its absence proves nothing.
    #[test]
    fn a_capped_default_list_proves_nothing() {
        let flatten = || failing("attributes", &["c/0"], &json!({ "outcome": reject() }), &accept());
        let clean = || failing("attributes", &[], &accept(), &accept());
        let mut capped = clean();
        capped["sections"]["attributes"]["gating_failure_count"] = 101.into();
        let mut reports = reports(vec![("a", capped, flatten()), ("b", clean(), flatten())]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "b")["gate"], "fail");
    }

    /// A set with no default run to compare with leaves the second proof
    /// without its evidence.
    #[test]
    fn a_missing_default_run_proves_nothing() {
        let flatten = || failing("attributes", &["c/0"], &json!({ "outcome": reject() }), &accept());
        let clean = || failing("attributes", &[], &accept(), &accept());
        let mut reports = reports(vec![("a", clean(), flatten()), ("b", clean(), flatten())]);
        reports.remove(&("portable".to_owned(), "b".to_owned()));
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// A harness failure says nothing about a case, so it neither vouches for
    /// nor withdraws the second proof, in either build, for a backend or for
    /// a whole run.
    #[test]
    fn a_harness_failure_does_not_withdraw_the_second_proof() {
        let flatten = || failing("attributes", &["c/0"], &json!({ "outcome": reject() }), &accept());
        let clean = || failing("attributes", &[], &accept(), &accept());
        let errored = || json!({ "gate": "error", "error": "wait failed", "gating_failures": [], "sections": {} });
        let run_error = json!({ "error": "conform exited non-zero" });
        let cases = [
            (
                "the backend, with arbitrary_precision",
                ("portable", "c+arbitrary-precision"),
                None,
            ),
            ("the backend, by default", ("portable", "c"), None),
            (
                "the run, with arbitrary_precision",
                ("portable", "c+arbitrary-precision"),
                Some(run_error.clone()),
            ),
            ("the run, by default", ("portable", "c"), Some(run_error)),
        ];
        for (what, (variant, name), run) in cases {
            let mut reports = reports(vec![
                ("a", clean(), flatten()),
                ("b", clean(), flatten()),
                ("c", clean(), flatten()),
            ]);
            reports.insert(
                (variant.to_owned(), name.to_owned()),
                run.unwrap_or_else(|| json!({ "error": "conform exited non-zero", "backends": { "c": errored() } })),
            );
            excuse_reference_moves(&mut reports);
            assert_eq!(ap_entry(&reports, "a")["gate"], "pass", "{what}");
            assert_eq!(ap_entry(&reports, "b")["gate"], "pass", "{what}");
        }
    }

    /// A set the harness lost does not count towards the two sets the second
    /// proof needs: one measured set alone still proves nothing.
    #[test]
    fn a_harness_failure_does_not_stand_in_for_a_second_set() {
        let flatten = || failing("attributes", &["c/0"], &json!({ "outcome": reject() }), &accept());
        let clean = || failing("attributes", &[], &accept(), &accept());
        let errored = json!({ "gate": "error", "error": "wait failed", "gating_failures": [], "sections": {} });
        let mut reports = reports(vec![("a", clean(), flatten()), ("b", clean(), errored)]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// A passing backend still blocks the proof beside an errored one.
    #[test]
    fn a_harness_failure_does_not_excuse_past_a_passing_set() {
        let flatten = || failing("attributes", &["c/0"], &json!({ "outcome": reject() }), &accept());
        let clean = || failing("attributes", &[], &accept(), &accept());
        let errored = json!({ "gate": "error", "error": "wait failed", "gating_failures": [], "sections": {} });
        let mut reports = reports(vec![
            ("a", clean(), flatten()),
            ("b", clean(), clean()),
            ("c", clean(), errored),
        ]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// A panic is the backend's defect whatever serde_json answers.
    #[test]
    fn a_panic_on_a_moved_case_stays() {
        let mut reports = reports(vec![
            (
                "a",
                failing("numbers", &[], &reject(), &accept()),
                failing("numbers", &["x#value"], &accept(), &json!({ "panic": "boom" })),
            ),
            (
                "b",
                failing("numbers", &["x#value"], &reject(), &accept()),
                failing("numbers", &[], &accept(), &accept()),
            ),
        ]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
    }

    /// A panic or crash named only by the failure detail stays too: these
    /// fixtures list no `panic` in `differences`, as the suite's encode, random
    /// and depth failures do not, or as a capped listing would not. The second
    /// set is excused on the same case, so the case itself did move.
    #[test]
    fn a_panic_or_crash_in_the_detail_stays() {
        for detail in [
            "panicked: boom",
            "encode panicked: boom",
            "decoded {\"error\":\"panicked: boom\"}, neither the correctly rounded value nor serde_json's",
            "crashed: signal 11",
        ] {
            let failed = || failing("numbers", &["x#value"], &reject(), &accept());
            let clean = || failing("numbers", &[], &accept(), &accept());
            let mut own = failed();
            own["gating_failures"][0]["detail"] = detail.into();
            own["sections"]["numbers"]["differences"] = json!([]);
            let mut reports = reports(vec![("a", clean(), own), ("b", clean(), failed())]);
            excuse_reference_moves(&mut reports);
            assert_eq!(ap_entry(&reports, "a")["gate"], "fail", "{detail}");
            assert_eq!(ap_entry(&reports, "b")["gate"], "pass", "{detail}");
        }
    }

    /// Failures past the listing cap still gate once every listed one is
    /// excused.
    #[test]
    fn unlisted_failures_keep_the_gate() {
        let mut capped = failing("numbers", &["x#value"], &accept(), &reject());
        capped["sections"]["numbers"]["gating_failure_count"] = 5.into();
        let mut reports = reports(vec![
            ("a", failing("numbers", &[], &reject(), &accept()), capped),
            (
                "b",
                failing("numbers", &["x#value"], &reject(), &accept()),
                failing("numbers", &[], &accept(), &accept()),
            ),
        ]);
        excuse_reference_moves(&mut reports);
        let a = ap_entry(&reports, "a");
        assert_eq!(a["gate"], "fail");
        assert_eq!(a["sections"]["numbers"]["gating_failure_count"], 4);
    }

    /// Every backend's suite crashing with `arbitrary_precision` is not
    /// serde_json changing an answer: a crash records no section to compare.
    #[test]
    fn a_crashed_suite_stays() {
        let crashed = || {
            json!({ "gate": "fail", "sections": {},
                "gating_failures": [{ "section": "process", "case": "suite", "detail": "signal 11" }] })
        };
        let clean = || failing("numbers", &[], &accept(), &accept());
        let mut reports = reports(vec![("a", clean(), crashed()), ("b", clean(), crashed())]);
        excuse_reference_moves(&mut reports);
        assert_eq!(ap_entry(&reports, "a")["gate"], "fail");
        assert_eq!(ap_entry(&reports, "b")["gate"], "fail");
    }

    /// The default builds, at every variant, gate what they gated.
    #[test]
    fn only_the_arbitrary_precision_run_at_portable_is_excused() {
        let moved_default = failing("numbers", &["x#value"], &reject(), &accept());
        let mut reports = reports(vec![
            (
                "a",
                failing("numbers", &[], &reject(), &accept()),
                failing("numbers", &["x#value"], &accept(), &reject()),
            ),
            (
                "b",
                moved_default.clone(),
                failing("numbers", &[], &accept(), &accept()),
            ),
        ]);
        reports.insert(
            ("native".to_owned(), "b".to_owned()),
            json!({ "backends": { "b": moved_default.clone() } }),
        );
        excuse_reference_moves(&mut reports);
        assert_eq!(
            reports[&("native".to_owned(), "b".to_owned())]["backends"]["b"],
            moved_default
        );
        assert_eq!(
            reports[&("portable".to_owned(), "b".to_owned())]["backends"]["b"],
            moved_default
        );
    }

    fn source(name: &str, host: Option<&str>) -> Source {
        Source {
            name: name.to_owned(),
            provenance: json!({ "git_dirty": false, "fingerprint": "f", "host": host }),
            skipped: None,
        }
    }

    #[test]
    fn stale_rejects_a_source_from_another_host() {
        let sources = [
            source("x86_64-linux-cpu/alloc.json", Some(HOST)),
            source("callgrind/x86_64.json", Some("x86_64-linux-other-cpu")),
            source("fuzz.json", None),
            Source {
                name: "miri.json".to_owned(),
                skipped: Some("not run".to_owned()),
                ..Source::default()
            },
        ];
        assert_eq!(
            stale(&sources, Some("f"), HOST),
            [
                "callgrind/x86_64.json: measured on host x86_64-linux-other-cpu (results are for x86_64-linux-cpu)",
                "fuzz.json: measured on host without a host stamp (results are for x86_64-linux-cpu)",
            ]
        );
    }

    #[test]
    fn stale_accepts_one_host_and_fingerprint() {
        let sources = [source("a.json", Some(HOST)), source("b.json", Some(HOST))];
        assert!(stale(&sources, Some("f"), HOST).is_empty());
    }
}
