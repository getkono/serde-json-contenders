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
    fn of(doc: &Value) -> Self {
        let rows = doc["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| (Key::of(r), r.clone()))
            .collect();
        Self {
            rows,
            provenance: doc["provenance"].clone(),
        }
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
        Ok(self.doc(name)?.map(|d| Table::of(&d)).unwrap_or_default())
    }

    fn rows(&mut self, name: &str) -> Result<Vec<Value>> {
        Ok(self
            .doc(name)?
            .and_then(|d| d["rows"].as_array().cloned())
            .unwrap_or_default())
    }
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
        data.sources = load.sources;
        Ok(data)
    }

    /// The host whose results the tables quote.
    pub fn primary(&self) -> Option<&Host> {
        (!self.host.slug.is_empty()).then_some(&self.host)
    }
}

fn load_host(load: &mut Loader, slug: &str) -> Result<Host> {
    let at = |file: &str| format!("{slug}/{file}");
    let e2e_time = load.rows(&at("e2e-time.json"))?;
    let mut conformance = BTreeMap::new();
    for variant in crate::matrix::variants() {
        for set in crate::matrix::SETS {
            let mut names = vec![set.name.to_owned()];
            if variant.name == "portable" {
                names.push(format!("{}+arbitrary-precision", set.name));
            }
            for name in names {
                if let Some(doc) = load.doc(&at(&format!("conformance/{}/{name}.json", variant.name)))? {
                    conformance.insert((variant.name.to_owned(), name), doc["report"].clone());
                }
            }
        }
    }
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
        doctor: load.doc(&at("doctor.json"))?.unwrap_or(Value::Null),
        conformance,
        provenance,
    })
}

/// Why each source cannot be trusted for the code at HEAD: measured on other
/// code (another fingerprint), or from a dirty checkout. Empty when every
/// source was measured on exactly the committed code.
#[must_use]
pub fn stale(sources: &[Source], fingerprint: Option<&str>) -> Vec<String> {
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
/// backend's own rejection is present; a row recording an error or a wrong
/// result is a failure of the run), every set in the cost tables, every
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
    gaps.extend(end_to_end(
        &data.e2e_count,
        "callgrind/e2e-x86_64.json",
        COUNTED_VARIANTS,
        false,
    ));
    gaps.extend(end_to_end(&host.e2e_time, "e2e-time.json", &["native"], true));
    let r = &data.recount;
    if !r.is_null() && (r["tolerance"] != 0.0 || r["compared"] != r["committed"] || r["committed"] == 0) {
        gaps.push(format!(
            "callgrind/recount.json: {} of {} counts recounted at tolerance {}; the check needs all of them at 0",
            r["compared"], r["committed"], r["tolerance"]
        ));
    }
    if let Some(f) = host.doctor["failures"].as_array().filter(|f| !f.is_empty()) {
        gaps.push(format!(
            "doctor.json: {} builds do not run the path they claim (first: {})",
            f.len(),
            f[0]
        ));
    }
    gaps
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
                        for s in shapes(w) {
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
                                Some(r) if r["error"].is_string() || r["verdict"] == "wrong" => "failed",
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
            "{name}: {missing} rows missing, {failed} failed or wrong (first {})",
            first.unwrap_or_default()
        )
    })
}

/// The sets the per-set tables have no row for.
fn per_set(data: &Data) -> Vec<String> {
    use crate::matrix::SETS;
    let host = &data.host;
    let mut gaps = Vec::new();
    for (name, rows, per_variant) in [
        ("size.json", &host.size, true),
        ("compile-time.json", &host.compile, false),
        ("msrv.json", &host.msrv, false),
        ("footprint.json", &data.footprint, false),
        ("miri.json", &data.miri, false),
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
        if !lacking.is_empty() && !rows.is_empty() {
            gaps.push(format!("{name}: no row for {}", lacking.join(", ")));
        }
    }
    gaps
}

/// Fuzz runs that are missing or did not finish, and Miri runs that failed
/// to run at all.
fn fuzz_and_miri(data: &Data) -> Vec<String> {
    use crate::fuzzing::{FUZZ_TARGETS, configs};
    use crate::matrix::SETS;
    let mut gaps = Vec::new();
    let lacking: Vec<String> = SETS
        .iter()
        .flat_map(|s| {
            configs(s.name)
                .iter()
                .flat_map(move |(c, _)| FUZZ_TARGETS.iter().map(move |t| (s.name, *c, *t)))
        })
        .filter(|(set, c, t)| {
            !data
                .fuzz
                .iter()
                .any(|r| r["set"] == *set && r["config"] == *c && r["target"] == *t)
        })
        .map(|(set, c, t)| format!("{set} {c} {t}"))
        .collect();
    if !lacking.is_empty() && !data.fuzz.is_empty() {
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
    if rows.is_empty() {
        return None;
    }
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
                        if row.is_none_or(|r| r["error"].is_string()) {
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
