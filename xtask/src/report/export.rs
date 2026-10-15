//! Every row of every result file, as CSV under `report/data/`.
//!
//! The report's tables are views; these files are the whole of what was
//! measured, generated from `results/` like everything else under
//! `report/`, so nothing measured goes unpublished. A nested value (a
//! round's figures, a crate list) is written as its JSON.

use std::collections::BTreeSet;

use serde_json::Value;

use super::data::Data;

/// One exported file: its name under `report/data/`, what it holds, and its
/// text.
pub struct Export {
    pub name: String,
    pub what: String,
    pub rows: usize,
    pub csv: String,
}

/// Every export, in a fixed order.
pub fn exports(data: &Data) -> Vec<Export> {
    let host = &data.host;
    let table = |t: &super::data::Table| t.rows.values().cloned().collect::<Vec<_>>();
    let mut out = vec![
        export(
            "callgrind.csv",
            "callgrind counts per operation (`results/callgrind/x86_64.json`)",
            &table(&data.callgrind),
        ),
        export(
            "e2e-count.csv",
            "callgrind counts per HTTP request (`results/callgrind/e2e-x86_64.json`)",
            &data.e2e_count,
        ),
        export(
            "hw.csv",
            "hardware cycles and instructions, every round (`hw.json`)",
            &table(&host.hw),
        ),
        export(
            "alloc.csv",
            "allocations per operation (`alloc.json`)",
            &table(&host.alloc),
        ),
        export(
            "time.csv",
            "wall-clock time per operation, every round (`time.json`)",
            &table(&host.time),
        ),
        export(
            "e2e-time.csv",
            "end-to-end throughput and latency, every rep (`e2e-time.json`)",
            &host.e2e_time,
        ),
        export(
            "size.csv",
            "`.text` of each fixture, every variant (`size.json`)",
            &host.size,
        ),
        export("compile-time.csv", "build times (`compile-time.json`)", &host.compile),
        export(
            "msrv.csv",
            "Rust 1.85 check and declared MSRV (`msrv.json`)",
            &host.msrv,
        ),
        export(
            "footprint.csv",
            "crates added, `unsafe` counts, advisories (`footprint.json`)",
            &data.footprint,
        ),
        export("fuzz.csv", "every fuzz run (`fuzz.json`)", &data.fuzz),
        export("miri.csv", "every Miri run (`miri.json`)", &data.miri),
        export(
            "doctor.csv",
            "what each build compiled and runs (`doctor.json`)",
            host.doctor["builds"].as_array().map_or(&[][..], Vec::as_slice),
        ),
        export(
            "doctor-failures.csv",
            "each build that does not run the path it claims (`doctor.json`)",
            &host.doctor["failures"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| serde_json::json!({ "failure": f }))
                .collect::<Vec<_>>(),
        ),
        export(
            "recount.csv",
            "the zero-tolerance recount of the callgrind counts (`callgrind/recount.json`)",
            &[serde_json::json!({
                "compared": data.recount["compared"],
                "committed": data.recount["committed"],
                "tolerance": data.recount["tolerance"],
            })],
        ),
    ];
    let Conformance {
        summary,
        differences,
        measures,
        notes,
    } = conformance(data);
    out.push(export(
        "conformance-sections.csv",
        "conformance: cases and disagreements per section, every build (`conformance/*/*.json`)",
        &summary,
    ));
    out.push(export(
        "conformance-differences.csv",
        "conformance: each recorded difference (a section records its first 50; the counts above are complete)",
        &differences,
    ));
    out.push(export(
        "conformance-data.csv",
        "conformance: every figure a section measured beyond its cases (float rounding tallies, borrow probes, line and column reporting, ...), one row per figure",
        &measures,
    ));
    out.push(export(
        "conformance-notes.csv",
        "conformance: every note a section recorded, and every gating failure listed",
        &notes,
    ));
    out
}

/// The conformance reports, flattened.
struct Conformance {
    summary: Vec<Value>,
    differences: Vec<Value>,
    measures: Vec<Value>,
    notes: Vec<Value>,
}

/// Each leaf of `v` under `path` (objects joined with `.`; an array is a
/// leaf, written as its JSON).
fn leaves(path: &str, v: &Value, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(o) => {
            for (k, child) in o {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                leaves(&p, child, out);
            }
        }
        leaf => out.push((path.to_owned(), leaf.clone())),
    }
}

/// Conformance reports flattened: one row per build, backend and section,
/// one per recorded difference, one per measured figure, and one per note.
fn conformance(data: &Data) -> Conformance {
    let mut summary = Vec::new();
    let mut differences = Vec::new();
    let mut measures = Vec::new();
    let mut notes = Vec::new();
    for ((variant, set), report) in &data.host.conformance {
        let Some(backends) = report["backends"].as_object() else {
            continue;
        };
        for (backend, b) in backends {
            let Some(sections) = b["sections"].as_object() else {
                continue;
            };
            for (section, s) in sections {
                summary.push(serde_json::json!({
                    "variant": variant, "set": set, "backend": backend, "section": section,
                    "gate": b["gate"], "gating": s["gating"], "cases": s["cases"],
                    "disagreements": s["disagreements"], "gating_failures": s["gating_failure_count"],
                }));
                for d in s["differences"].as_array().into_iter().flatten() {
                    differences.push(serde_json::json!({
                        "variant": variant, "set": set, "backend": backend, "section": section,
                        "case": d["case"], "backend_outcome": d["backend"], "reference_outcome": d["reference"],
                        "detail": d["detail"],
                    }));
                }
                let mut figures = Vec::new();
                leaves("", &s["data"], &mut figures);
                for (figure, value) in figures {
                    measures.push(serde_json::json!({
                        "variant": variant, "set": set, "backend": backend, "section": section,
                        "figure": figure, "value": value,
                    }));
                }
                for f in b["gating_failures"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|f| f["section"] == *section)
                {
                    notes.push(serde_json::json!({
                        "variant": variant, "set": set, "backend": backend, "section": section,
                        "note": format!("gating failure: {}", f),
                    }));
                }
                for note in s["notes"].as_array().into_iter().flatten() {
                    notes.push(serde_json::json!({
                        "variant": variant, "set": set, "backend": backend, "section": section, "note": note,
                    }));
                }
            }
        }
    }
    Conformance {
        summary,
        differences,
        measures,
        notes,
    }
}

/// `rows` as CSV: the columns every row has between them, sorted, with the
/// identifying ones first.
fn export(name: &str, what: &str, rows: &[Value]) -> Export {
    const FIRST: &[&str] = &[
        "variant", "set", "backend", "workload", "op", "form", "arrival", "route", "section", "case", "config",
        "target",
    ];
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for r in rows {
        if let Value::Object(o) = r {
            keys.extend(o.keys().cloned());
        }
    }
    let mut columns: Vec<String> = FIRST
        .iter()
        .filter(|k| keys.contains(**k))
        .map(|k| (*k).to_owned())
        .collect();
    columns.extend(keys.into_iter().filter(|k| !FIRST.contains(&k.as_str())));
    let mut csv = columns.iter().map(|c| field(c)).collect::<Vec<_>>().join(",");
    csv.push('\n');
    for r in rows {
        let line: Vec<String> = columns
            .iter()
            .map(|c| match &r[c.as_str()] {
                Value::Null => String::new(),
                Value::String(s) => field(s),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                other => field(&other.to_string()),
            })
            .collect();
        csv.push_str(&line.join(","));
        csv.push('\n');
    }
    Export {
        name: name.to_owned(),
        what: what.to_owned(),
        rows: rows.len(),
        csv,
    }
}

/// One CSV field, quoted when it must be (RFC 4180).
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::export;

    #[test]
    fn rows_become_csv_with_identity_first_and_nested_values_as_json() {
        let rows = [
            json!({ "cycles": 10.5, "backend": "sonic-rs", "rounds": [1, 2], "detail": "a, \"b\"" }),
            json!({ "backend": "serde_json", "cycles": 12 }),
        ];
        let e = export("x.csv", "x", &rows);
        assert_eq!(
            e.csv,
            "backend,cycles,detail,rounds\nsonic-rs,10.5,\"a, \"\"b\"\"\",\"[1,2]\"\nserde_json,12,,\n"
        );
        assert_eq!(e.rows, 2);
    }
}
