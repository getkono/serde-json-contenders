//! Loading `results/` into lookups.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
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
    fn load(path: &Path) -> Self {
        let Ok(doc) = read_json(path) else {
            return Self::default();
        };
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

/// Plain row lists (size, compile time, e2e...).
fn rows(path: &Path) -> (Vec<Value>, Value) {
    read_json(path).map_or_else(
        |_| (Vec::new(), Value::Null),
        |doc| {
            (
                doc["rows"].as_array().cloned().unwrap_or_default(),
                doc["provenance"].clone(),
            )
        },
    )
}

/// One host's results.
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
    pub e2e_trust: Option<String>,
    pub doctor: Value,
    /// conformance reports: (variant, set) -> report.
    pub conformance: BTreeMap<(String, String), Value>,
    pub provenance: Value,
}

impl Host {
    fn load(dir: &Path) -> Self {
        let slug = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (size, _) = rows(&dir.join("size.json"));
        let (compile, _) = rows(&dir.join("compile-time.json"));
        let (msrv, _) = rows(&dir.join("msrv.json"));
        let (e2e_time, _) = rows(&dir.join("e2e-time.json"));
        let e2e_trust = e2e_time.first().and_then(|r| r["trust"].as_str()).map(str::to_owned);
        let mut conformance = BTreeMap::new();
        if let Ok(variants) = std::fs::read_dir(dir.join("conformance")) {
            for variant in variants.filter_map(Result::ok) {
                for file in std::fs::read_dir(variant.path())
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                {
                    if let Ok(doc) = read_json(&file.path()) {
                        let set = file
                            .path()
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        conformance.insert(
                            (variant.file_name().to_string_lossy().into_owned(), set),
                            doc["report"].clone(),
                        );
                    }
                }
            }
        }
        let alloc = Table::load(&dir.join("alloc.json"));
        let provenance = alloc.provenance.clone();
        Self {
            slug,
            hw: Table::load(&dir.join("hw.json")),
            time: Table::load(&dir.join("time.json")),
            alloc,
            size,
            compile,
            msrv,
            e2e_time,
            e2e_trust,
            doctor: read_json(&dir.join("doctor.json")).unwrap_or(Value::Null),
            conformance,
            provenance,
        }
    }

    pub fn arch(&self) -> &str {
        self.slug.split('-').next().unwrap_or_default()
    }

    pub fn os(&self) -> &str {
        self.slug.split('-').nth(1).unwrap_or_default()
    }
}

/// Everything under `results/`.
#[derive(Debug, Default)]
pub struct Data {
    /// callgrind by architecture.
    pub callgrind: BTreeMap<String, Table>,
    pub e2e_count: BTreeMap<String, Vec<Value>>,
    pub hosts: Vec<Host>,
    pub footprint: Vec<Value>,
    pub advisory_db: String,
    pub fuzz: Vec<Value>,
    pub miri: Vec<Value>,
    pub soundness: toml::Table,
    pub notes: toml::Table,
}

impl Data {
    pub fn load() -> Result<Self> {
        let results = root().join("results");
        let mut data = Self::default();
        for arch in ["x86_64", "aarch64"] {
            let table = Table::load(&results.join("callgrind").join(format!("{arch}.json")));
            if !table.is_empty() {
                data.callgrind.insert(arch.into(), table);
            }
            let (e2e, _) = rows(&results.join("callgrind").join(format!("e2e-{arch}.json")));
            if !e2e.is_empty() {
                data.e2e_count.insert(arch.into(), e2e);
            }
        }
        let mut hosts: Vec<Host> = std::fs::read_dir(&results)?
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir() && e.file_name() != "callgrind")
            .map(|e| Host::load(&e.path()))
            .collect();
        hosts.sort_by(|a, b| a.slug.cmp(&b.slug));
        data.hosts = hosts;
        let footprint = read_json(&results.join("footprint.json")).unwrap_or(Value::Null);
        data.footprint = footprint["rows"].as_array().cloned().unwrap_or_default();
        footprint["advisory_db"]
            .as_str()
            .unwrap_or_default()
            .clone_into(&mut data.advisory_db);
        data.fuzz = rows(&results.join("fuzz.json")).0;
        data.miri = rows(&results.join("miri.json")).0;
        let toml = |name: &str| {
            std::fs::read_to_string(results.join(name))
                .ok()
                .and_then(|t| t.parse::<toml::Table>().ok())
                .unwrap_or_default()
        };
        data.soundness = toml("soundness.toml");
        data.notes = toml("notes.toml");
        Ok(data)
    }

    /// The x86-64 Linux host whose host-specific results (allocations,
    /// hardware counters, size, compile time) the tables quote.
    pub fn primary(&self) -> Option<&Host> {
        self.hosts
            .iter()
            .find(|h| h.arch() == "x86_64" && h.os() == "linux")
            .or_else(|| self.hosts.first())
    }
}
