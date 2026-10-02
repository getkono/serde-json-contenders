//! Markdown for the README's results section.

use std::fmt::Write as _;

use serde_json::Value;

use super::data::{Data, Key};
use super::verdict::{
    self, ARM_NATIVE, ENTRIES, Entry, Outcome, POINTS, Point, Verdict, X86_NATIVE, X86_V3, cpu, heap,
};
use crate::matrix::KYNOS;

fn ratio(mine: Option<f64>, base: Option<f64>) -> String {
    match (mine, base) {
        (Some(m), Some(b)) if b > 0.0 => format!("{:.2}×", m / b),
        _ => "—".into(),
    }
}

fn num(v: Option<f64>) -> String {
    match v {
        Some(v) if v >= 1e6 => format!("{:.2} M", v / 1e6),
        Some(v) if v >= 1e4 => format!("{:.1} k", v / 1e3),
        Some(v) => format!("{v:.0}"),
        None => "—".into(),
    }
}

fn mark(o: &Outcome) -> &'static str {
    match o {
        Outcome::Pass(_) => "✅",
        Outcome::Fail(_) => "❌",
        Outcome::Pending(_) => "⏳",
    }
}

fn row(cells: &[String]) -> String {
    format!("| {} |\n", cells.join(" | "))
}

fn header(cells: &[&str]) -> String {
    let mut s = row(&cells.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>());
    s.push_str(&row(&cells.iter().map(|_| "---".to_owned()).collect::<Vec<_>>()));
    s
}

fn has_point(data: &Data, p: Point) -> bool {
    cpu(data, p, "serde_json", "json-small", "decode").is_some()
}

/// The README results section.
pub fn render(data: &Data, verdicts: &[Verdict]) -> String {
    let mut out = String::new();
    verdict_table(&mut out, verdicts);
    rules_table(&mut out, verdicts);
    kynos_cpu(&mut out, data);
    sweep(&mut out, data);
    families(&mut out, data);
    heap_table(&mut out, data);
    arrival(&mut out, data);
    encode_into(&mut out, data);
    costs(&mut out, data);
    conformance(&mut out, data);
    depth_limits(&mut out, data);
    end_to_end(&mut out, data);
    timed(&mut out, data);
    let notes = limitations(data);
    if !notes.is_empty() {
        out.push_str("### Limitations\n\n");
        for note in notes {
            let _ = writeln!(out, "- {note}");
        }
        out.push('\n');
    }
    provenance(&mut out, data);
    out
}

/// What the recorded hosts and run lengths leave unmeasured, each derived from
/// `results/` so a new host or a longer run retires its own note.
fn limitations(data: &Data) -> Vec<String> {
    let mut notes = Vec::new();
    if data.hosts.is_empty() {
        return notes;
    }
    let x86: Vec<&str> = data
        .hosts
        .iter()
        .filter(|h| h.arch() == "x86_64")
        .map(|h| h.provenance["cpu"].as_str().unwrap_or(&h.slug))
        .collect();
    if !x86.iter().any(|cpu| cpu.to_lowercase().contains("intel")) {
        notes.push(if x86.is_empty() {
            "No x86-64 host is recorded, so no Intel host either.".to_owned()
        } else {
            format!(
                "No Intel host: every host-measured x86-64 number is from {}.",
                x86.join(", ")
            )
        });
    }
    if !data.hosts.iter().any(|h| h.os() == "macos") {
        notes.push(
            "No macOS host: macOS timed and allocation numbers are pending a rerun on a quiet machine.".to_owned(),
        );
    }
    let mut classes: Vec<String> = data
        .hosts
        .iter()
        .flat_map(|h| h.time.rows.values().chain(h.e2e_time.iter()))
        .map(|r| r["trust"].as_str().unwrap_or("?").to_owned())
        .collect();
    classes.sort();
    classes.dedup();
    if classes.iter().all(|c| c == "solo") && !classes.is_empty() {
        let load = data
            .hosts
            .iter()
            .filter_map(|h| h.time.provenance["loadavg"].as_str())
            .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>();
        notes.push(format!(
            "Every timed number (timed sampling and end to end) is `solo` class: taken on a shared machine{}, not a quieted one, so a difference inside its A/A band is noise.",
            if load.is_empty() {
                String::new()
            } else {
                format!(" (1, 5 and 15 minute load average {} as timed sampling finished)", load.join("; "))
            }
        ));
    }
    for h in &data.hosts {
        let mut runs = Vec::new();
        let rounds = h
            .time
            .rows
            .values()
            .filter_map(|r| r["rounds"].as_array().map(Vec::len))
            .min();
        if let Some(n) = rounds {
            runs.push(format!(
                "timed sampling {}",
                counted_run(n as u64, " rounds", crate::timed::ROUNDS)
            ));
        }
        let reps = h
            .e2e_time
            .iter()
            .filter_map(|r| r["reps"].as_array().map(Vec::len))
            .min();
        let secs = h.e2e_time.iter().filter_map(|r| r["seconds"].as_u64()).min();
        if let (Some(n), Some(s)) = (reps, secs) {
            runs.push(format!(
                "end to end {} of {} per load phase",
                counted_run(n as u64, " reps", crate::endtoend::REPS),
                counted_run(s, " s", crate::endtoend::SECONDS)
            ));
        }
        let builds = h.compile.iter().filter_map(|r| r["reps"].as_u64()).min();
        if let Some(n) = builds {
            runs.push(format!(
                "compile time {}",
                counted_run(n, " builds", crate::cost::COMPILE_REPS as u64)
            ));
        }
        if !runs.is_empty() {
            notes.push(format!("Run lengths on {}: {}.", h.slug, runs.join("; ")));
        }
    }
    if let Some(s) = data.fuzz.iter().filter_map(|r| r["seconds"].as_u64()).min() {
        notes.push(format!(
            "Fuzzing ran {} per target and configuration.",
            counted_run(s, " s", crate::fuzzing::SECONDS)
        ));
    }
    notes
}

/// `n` and its unit, flagged when `n` is below the collector's default.
fn counted_run(n: u64, unit: &str, default: u64) -> String {
    if n < default {
        format!("{n}{unit} (reduced from the default {default})")
    } else {
        format!("{n}{unit}")
    }
}

fn verdict_table(out: &mut String, verdicts: &[Verdict]) {
    out.push_str("### Verdict\n\n");
    out.push_str(&header(&[
        "Entry point",
        "Crate",
        "Add to Kynos?",
        "Deserves to exist?",
        "Why",
    ]));
    for v in verdicts {
        let why = v.recommended.detail().to_owned();
        let add = if v.entry.backend == "serde_json" {
            "baseline".to_owned()
        } else {
            format!("{} {}", mark(&v.recommended), v.recommended.word())
        };
        out.push_str(&row(&[
            format!("`{}`", v.entry.backend),
            v.entry.krate.to_owned(),
            add,
            format!("{} — {}", if v.exists.0 { "yes" } else { "no" }, v.exists.1),
            why,
        ]));
    }
    out.push('\n');
}

fn rules_table(out: &mut String, verdicts: &[Verdict]) {
    out.push_str("### The decision rule, entry by entry\n\n");
    out.push_str(&header(&[
        "Entry point",
        "1 Frontier",
        "2 Conformance",
        "3 Soundness",
        "4 End to end",
    ]));
    for v in verdicts.iter().filter(|v| !v.entry.backend.starts_with("serde_json")) {
        let cell = |o: &Outcome| format!("{} {}", mark(o), o.detail());
        out.push_str(&row(&[
            format!("`{}`", v.entry.backend),
            cell(&v.rules[0]),
            cell(&v.rules[1]),
            cell(&v.rules[2]),
            cell(&v.rules[3]),
        ]));
    }
    out.push('\n');
}

fn cpu_block(
    out: &mut String,
    data: &Data,
    title: &str,
    workloads: &[&str],
    op: &str,
    points: &[Point],
    entries: &[&Entry],
) {
    let points: Vec<Point> = points.iter().copied().filter(|p| has_point(data, *p)).collect();
    if points.is_empty() {
        return;
    }
    let _ = writeln!(out, "{title}\n");
    let mut cols = vec!["Workload".to_owned(), "Point".to_owned(), "serde_json".to_owned()];
    cols.extend(entries.iter().skip(1).map(|e| format!("`{}`", e.backend)));
    let refs: Vec<&str> = cols.iter().map(String::as_str).collect();
    out.push_str(&header(&refs));
    for w in workloads {
        for p in &points {
            let base = cpu(data, *p, "serde_json", w, op);
            let mut cells = vec![(*w).to_owned(), p.label(), num(base)];
            cells.extend(
                entries
                    .iter()
                    .skip(1)
                    .map(|e| ratio(cpu(data, *p, e.backend, w, op), base)),
            );
            out.push_str(&row(&cells));
        }
    }
    out.push('\n');
}

fn all_entries() -> Vec<&'static Entry> {
    ENTRIES.iter().collect()
}

fn encoders() -> Vec<&'static Entry> {
    ENTRIES.iter().filter(|e| e.encodes).collect()
}

fn kynos_cpu(out: &mut String, data: &Data) {
    out.push_str("### CPU per operation at the three Kynos shapes\n\n");
    out.push_str("serde_json is the absolute figure; every other column is a ratio to it (below 1 is faster). Owned decode of one shared frame, and `to_vec` encode: what a server does. The rule reads the cycle rows; the instruction rows beside them are the same builds counted as instructions.\n\n");
    let points = [
        X86_V3,
        X86_V3.instructions(),
        X86_NATIVE,
        X86_NATIVE.instructions(),
        ARM_NATIVE,
        ARM_NATIVE.instructions(),
    ];
    cpu_block(out, data, "**Decode**", KYNOS, "decode", &points, &all_entries());
    cpu_block(out, data, "**Encode**", KYNOS, "encode", &points, &encoders());
}

fn sweep(out: &mut String, data: &Data) {
    let sizes: Vec<&str> = payloads::ALL
        .iter()
        .copied()
        .filter(|w| w.starts_with("sweep-"))
        .collect();
    out.push_str("### Size sweep: where SIMD starts to pay\n\n");
    cpu_block(
        out,
        data,
        "**Decode**",
        &sizes,
        "decode",
        &[X86_V3, ARM_NATIVE],
        &all_entries(),
    );
    cpu_block(
        out,
        data,
        "**Encode**",
        &sizes,
        "encode",
        &[X86_V3, ARM_NATIVE],
        &encoders(),
    );
}

fn families(out: &mut String, data: &Data) {
    let fams: Vec<&str> = payloads::ALL
        .iter()
        .copied()
        .filter(|w| !KYNOS.contains(w) && !w.starts_with("sweep-"))
        .collect();
    out.push_str("### One variable at a time, and real-world shapes\n\n");
    out.push_str(
        "A `—` under a nested workload is a rejection at that depth (see conformance), not a missing run.\n\n",
    );
    cpu_block(out, data, "**Decode**", &fams, "decode", &[X86_V3], &all_entries());
    cpu_block(out, data, "**Encode**", &fams, "encode", &[X86_V3], &encoders());
}

fn heap_table(out: &mut String, data: &Data) {
    let Some(host) = data.primary() else { return };
    if host.alloc.is_empty() {
        return;
    }
    out.push_str("### Heap per operation\n\n");
    out.push_str("At `native`, owned decode of one shared frame and `to_vec` encode. Bytes are those requested, counting only the growth of a reallocation; peak is the most live at once.\n\n");
    out.push_str(&header(&[
        "Entry point",
        "Workload",
        "Decode allocations",
        "Decode reallocations",
        "Decode bytes",
        "Decode peak",
        "Encode allocations",
        "Encode reallocations",
        "Encode bytes",
        "Encode peak",
    ]));
    for e in ENTRIES {
        for w in KYNOS {
            let d = host.alloc.row(&Key::new("native", e.backend, w, "decode"));
            let n = host.alloc.row(&Key::new("native", e.backend, w, "encode"));
            let f = |r: Option<&Value>, k: &str| num(r.and_then(|r| r[k].as_f64()));
            out.push_str(&row(&[
                format!("`{}`", e.backend),
                (*w).to_owned(),
                f(d, "allocations"),
                f(d, "reallocations"),
                f(d, "bytes"),
                f(d, "peak_bytes"),
                f(n, "allocations"),
                f(n, "reallocations"),
                f(n, "bytes"),
                f(n, "peak_bytes"),
            ]));
        }
    }
    out.push('\n');
    let _ = heap;
}

fn arrival(out: &mut String, data: &Data) {
    let Some(table) = data.callgrind.get("x86_64") else {
        return;
    };
    out.push_str("### How the body arrives, and borrowing\n\n");
    out.push_str("x86-64-v3 estimated cycles for `json-large`, relative to the same backend's owned decode of a shared frame. In-place parsers pay a copy when the frame is shared; borrowing saves the string allocations.\n\n");
    out.push_str(&header(&[
        "Entry point",
        "owned, shared (abs.)",
        "owned, unique",
        "borrowed, shared",
        "borrowed, unique",
    ]));
    for e in ENTRIES {
        let get = |form: &str, arrival: &str| {
            let mut k = Key::new("v3", e.backend, "json-large", "decode");
            k.form = form.into();
            k.arrival = arrival.into();
            table.get(&k, "est_cycles")
        };
        let base = get("owned", "shared");
        out.push_str(&row(&[
            format!("`{}`", e.backend),
            num(base),
            ratio(get("owned", "unique"), base),
            ratio(get("borrowed", "shared"), base),
            ratio(get("borrowed", "unique"), base),
        ]));
    }
    out.push('\n');
}

/// `to_writer` into a reused buffer beside `to_vec` into a fresh one.
fn encode_into(out: &mut String, data: &Data) {
    let Some(table) = data.callgrind.get("x86_64") else {
        return;
    };
    let get = |backend: &str, workload: &str, op: &str| table.get(&Key::new("v3", backend, workload, op), "est_cycles");
    if get("serde_json", KYNOS[0], "encode-into").is_none() {
        return;
    }
    out.push_str("### Encoding into a reused buffer\n\n");
    out.push_str("x86-64-v3 estimated cycles: `to_vec` into a fresh `Vec`, and `to_writer` into a reused `Vec` that already has capacity, relative to the same backend's `to_vec`.\n\n");
    let mut cols = vec!["Entry point".to_owned()];
    for w in KYNOS {
        cols.push(format!("{w} `to_vec` (abs.)"));
        cols.push(format!("{w} `to_writer`"));
    }
    let refs: Vec<&str> = cols.iter().map(String::as_str).collect();
    out.push_str(&header(&refs));
    for e in encoders() {
        let mut cells = vec![format!("`{}`", e.backend)];
        for w in KYNOS {
            let base = get(e.backend, w, "encode");
            cells.push(num(base));
            cells.push(ratio(get(e.backend, w, "encode-into"), base));
        }
        out.push_str(&row(&cells));
    }
    out.push('\n');
}

fn costs(out: &mut String, data: &Data) {
    let Some(host) = data.primary() else { return };
    out.push_str("### What adopting it costs besides CPU\n\n");
    out.push_str(&header(&[
        "Set",
        "`.text` +full (native)",
        "`.text` +decode-only",
        "Clean release build",
        "Clean dev build",
        "Incremental dev build",
        "Builds on 1.85",
        "Declared MSRV",
        "Crates added",
        "`unsafe` blocks / fns / impls / traits",
        "Advisories",
    ]));
    for set in crate::matrix::SETS {
        let size = host
            .size
            .iter()
            .find(|r| r["set"] == set.name && r["variant"] == "native");
        let compile = host.compile.iter().find(|r| r["set"] == set.name);
        let msrv = host.msrv.iter().find(|r| r["set"] == set.name);
        let fp = data.footprint.iter().find(|r| r["set"] == set.name);
        let advisories: Vec<String> = fp
            .and_then(|f| f["crates"].as_array())
            .into_iter()
            .flatten()
            .flat_map(|c| c["advisories"].as_array().cloned().unwrap_or_default())
            .map(|a| {
                format!(
                    "{}{}",
                    a["id"].as_str().unwrap_or("?"),
                    if a["affects_pinned_version"] == true {
                        " (affects pin)"
                    } else {
                        " (patched)"
                    }
                )
            })
            .collect();
        let s = |r: Option<&Value>, k: &str| r.and_then(|r| r[k].as_f64());
        out.push_str(&row(&[
            set.name.to_owned(),
            s(size, "full_delta").map_or("—".into(), |v| format!("{:+.0} KiB", v / 1024.0)),
            s(size, "decode_delta").map_or("—".into(), |v| format!("{:+.0} KiB", v / 1024.0)),
            s(compile, "release_s").map_or("—".into(), |v| format!("{v:.1} s")),
            s(compile, "dev_s").map_or("—".into(), |v| format!("{v:.1} s")),
            s(compile, "incremental_s").map_or("—".into(), |v| format!("{v:.2} s")),
            msrv.map_or("—".into(), |m| {
                if m["builds_on_msrv"] == true {
                    "yes".into()
                } else {
                    "no".into()
                }
            }),
            msrv.and_then(|m| m["declared"].as_str()).unwrap_or("none").to_owned(),
            fp.map_or("—".into(), |f| f["crates_added"].to_string()),
            fp.map_or("—".into(), |f| {
                format!(
                    "{} / {} / {} / {}",
                    f["unsafe_blocks"], f["unsafe_fns"], f["unsafe_impls"], f["unsafe_traits"]
                )
            }),
            if advisories.is_empty() {
                "none".into()
            } else {
                advisories.join(", ")
            },
        ]));
    }
    out.push('\n');
    if !data.soundness.is_empty() {
        out.push_str("Open issues touching soundness, reviewed by hand (`results/soundness.toml`); none fail rule 3 unless listed as open:\n\n");
        for (krate, review) in &data.soundness {
            let list = |k: &str| {
                review
                    .get(k)
                    .and_then(toml::Value::as_array)
                    .map(|a| a.iter().filter_map(toml::Value::as_str).collect::<Vec<_>>().join("; "))
            };
            let open = list("open").filter(|s| !s.is_empty());
            let reviewed = list("reviewed").unwrap_or_default();
            let _ = writeln!(
                out,
                "- **{krate}**: {}{reviewed}",
                open.map_or(String::new(), |o| format!("OPEN: {o}. "))
            );
        }
        out.push('\n');
    }
}

fn conformance(out: &mut String, data: &Data) {
    let reports: Vec<(&String, &(String, String), &Value)> = data
        .hosts
        .iter()
        .flat_map(|h| h.conformance.iter().map(move |(k, v)| (&h.slug, k, v)))
        .collect();
    if reports.is_empty() {
        return;
    }
    out.push_str("### Conformance against serde_json\n\n");
    out.push_str("Disagreements per section, summed over every build variant and host; **bold** sections gate. Gating failures are listed below the table.\n\n");
    let sections = [
        "grammar",
        "status",
        "strings",
        "structs",
        "depth",
        "numbers",
        "attributes",
        "value_targets",
        "encoding",
        "encode_diff",
    ];
    let mut cols = vec!["Entry point", "Gate"];
    cols.extend(sections);
    out.push_str(&header(&cols));
    let mut listed = Vec::new();
    for e in ENTRIES
        .iter()
        .filter(|e| e.backend != "serde_json" && e.backend != "serde_json+float_roundtrip")
    {
        let mine: Vec<&Value> = reports
            .iter()
            .map(|(_, _, r)| &r["backends"][e.backend])
            .filter(|b| !b.is_null())
            .collect();
        if mine.is_empty() {
            continue;
        }
        let gate = if mine.iter().all(|b| b["gate"] == "pass") {
            "✅ pass"
        } else {
            "❌ fail"
        };
        let mut cells = vec![format!("`{}`", e.backend), gate.to_owned()];
        for s in sections {
            let n: u64 = mine
                .iter()
                .filter_map(|b| b["sections"][s]["disagreements"].as_u64())
                .sum();
            let gating = mine.iter().any(|b| b["sections"][s]["gating"] == true);
            cells.push(if gating && n > 0 {
                format!("**{n}**")
            } else {
                n.to_string()
            });
        }
        out.push_str(&row(&cells));
        for ((_, (variant, set), r), b) in reports
            .iter()
            .zip(reports.iter().map(|(_, _, r)| &r["backends"][e.backend]))
        {
            let _ = r;
            for f in b["gating_failures"].as_array().into_iter().flatten().take(5) {
                let line = format!(
                    "- `{}` ({variant}/{set}): {} — `{}`: {}",
                    e.backend,
                    f["section"].as_str().unwrap_or("?"),
                    f["case"].as_str().unwrap_or("?"),
                    f["detail"].as_str().unwrap_or("").chars().take(160).collect::<String>()
                );
                if !listed.iter().any(|l: &String| {
                    l.split(" (").next() == line.split(" (").next()
                        && l.ends_with(line.rsplit(" — ").next().unwrap_or(""))
                }) {
                    listed.push(line);
                }
            }
        }
    }
    out.push('\n');
    for l in listed.iter().take(60) {
        out.push_str(l);
        out.push('\n');
    }
    if !listed.is_empty() {
        out.push('\n');
    }
}

/// One depth limit: the deepest input accepted, and what happened next.
fn depth_limit(limit: &Value) -> String {
    let max = limit["max_accepted"]
        .as_u64()
        .map_or_else(|| "none".to_owned(), |m| m.to_string());
    if let Some(n) = limit["first_crashed"].as_u64() {
        format!("≤{max}, crashes at {n}")
    } else if let Some(n) = limit["first_rejected"].as_u64() {
        format!("≤{max}, rejects {n}")
    } else {
        format!("all, to {max}")
    }
}

/// The depth limits at one target and stack, arrays and objects together
/// where they agree.
fn depth_cell(limits: &Value, target: &str, stack: &str) -> String {
    let array = &limits[format!("array/{target}/{stack}")];
    let object = &limits[format!("object/{target}/{stack}")];
    match (array.is_null(), object.is_null()) {
        (true, true) => "—".into(),
        _ if array == object => depth_limit(array),
        _ => format!("array {}; object {}", depth_limit(array), depth_limit(object)),
    }
}

/// The depth section's limits, which it records but gates only on a crash.
fn depth_limits(out: &mut String, data: &Data) {
    let Some(host) = data.primary() else { return };
    let report = |set: &str| {
        ["native", "v3", "portable"].iter().find_map(|v| {
            host.conformance
                .get(&((*v).to_owned(), set.to_owned()))
                .map(|r| (*v, r))
        })
    };
    let limits = |e: &Entry| -> Option<(&str, &Value)> {
        if e.backend == "serde_json" {
            let (variant, r) = ENTRIES.iter().skip(1).find_map(|o| report(o.set))?;
            let b = r["backends"].as_object()?.values().next()?;
            return Some((variant, &b["sections"]["depth"]["data"]["limits"]["reference"]));
        }
        let (variant, r) = report(e.set)?;
        let l = &r["backends"][e.backend]["sections"]["depth"]["data"]["limits"]["backend"];
        (!l.is_null()).then_some((variant, l))
    };
    let rows: Vec<(&Entry, &str, &Value)> = ENTRIES
        .iter()
        .filter(|e| e.backend != "serde_json+float_roundtrip")
        .filter_map(|e| limits(e).map(|(v, l)| (e, v, l)))
        .collect();
    if rows.is_empty() {
        return;
    }
    out.push_str("**Nesting depth limits**, which the depth section records and gates only on a crash: the deepest document accepted, and the next depth tried, from 100 up to 1 000 000 levels, decoded into `Value` and into `IgnoredAny` on a 2 MiB (tokio's worker) and an 8 MiB stack.\n\n");
    out.push_str(&header(&[
        "Entry point",
        "Variant",
        "`Value`, 2 MiB",
        "`Value`, 8 MiB",
        "`IgnoredAny`, 2 MiB",
        "`IgnoredAny`, 8 MiB",
    ]));
    for (e, variant, l) in rows {
        out.push_str(&row(&[
            format!("`{}`", e.backend),
            variant.to_owned(),
            depth_cell(l, "value", "2MiB"),
            depth_cell(l, "value", "8MiB"),
            depth_cell(l, "ignored", "2MiB"),
            depth_cell(l, "ignored", "8MiB"),
        ]));
    }
    out.push('\n');
}

fn end_to_end(out: &mut String, data: &Data) {
    if data.e2e_count.is_empty() && data.hosts.iter().all(|h| h.e2e_time.is_empty()) {
        return;
    }
    out.push_str("### End to end: a hyper server\n\n");
    for (arch, rows) in &data.e2e_count {
        let variant = if arch == "aarch64" {
            ARM_NATIVE.variant
        } else {
            X86_V3.variant
        };
        let _ = writeln!(
            out,
            "**Counted, {arch} {variant}** — estimated cycles per request; codec share = (backend − floor) ÷ backend, where the floor serves the same routes with no JSON.\n"
        );
        out.push_str(&header(&[
            "Route",
            "Entry point",
            "Cycles / request",
            "vs serde_json",
            "Codec share",
        ]));
        let get = |b: &str, r: &str| {
            rows.iter()
                .find(|x| x["backend"] == b && x["route"] == r && x["variant"] == variant)
                .and_then(|x| x["est_cycles"].as_f64())
        };
        for (route, ..) in crate::endtoend::ROUTES {
            let floor = get("floor", route);
            let base = get("serde_json", route);
            for e in ENTRIES {
                let Some(me) = get(e.backend, route) else { continue };
                let share = floor.map_or("—".into(), |f| format!("{:.0} %", (me - f) / me * 100.0));
                out.push_str(&row(&[
                    (*route).to_owned(),
                    format!("`{}`", e.backend),
                    num(Some(me)),
                    ratio(Some(me), base),
                    share,
                ]));
            }
        }
        out.push('\n');
    }
    for host in data.hosts.iter().filter(|h| !h.e2e_time.is_empty()) {
        let _ = writeln!(
            out,
            "**Timed, {} ({})** — closed-loop throughput at 64 connections, and p99 open-loop at 70 % of serde_json's throughput; median of reps, relative to serde_json.\n",
            host.slug,
            host.e2e_trust.as_deref().unwrap_or("?")
        );
        out.push_str(&header(&[
            "Route",
            "Entry point",
            "Throughput",
            "p99",
            "A/A band (rps, p99)",
            "Counted prediction",
        ]));
        for (route, ..) in crate::endtoend::ROUTES {
            for e in ENTRIES.iter().skip(1) {
                let Some((rps, p99, aa_rps, aa_p99)) = verdict::e2e_gain(&host.e2e_time, e.backend, route) else {
                    continue;
                };
                let predicted = verdict::e2e_predicted(data, host.arch(), e.backend, route)
                    .map_or("—".into(), |p| format!("{:+.1} %", p * 100.0));
                out.push_str(&row(&[
                    (*route).to_owned(),
                    format!("`{}`", e.backend),
                    format!("{:+.1} %", rps * 100.0),
                    format!("{:+.1} %", -p99 * 100.0),
                    format!("±{:.1} %, ±{:.1} %", aa_rps * 100.0, aa_p99 * 100.0),
                    predicted,
                ]));
            }
        }
        out.push('\n');
    }
}

fn timed(out: &mut String, data: &Data) {
    for host in data.hosts.iter().filter(|h| !h.time.is_empty()) {
        let trust = host
            .time
            .rows
            .values()
            .next()
            .and_then(|r| r["trust"].as_str())
            .unwrap_or("?")
            .to_owned();
        let _ = writeln!(out, "### Timed, {} ({trust})\n", host.slug);
        out.push_str("Median ns per operation at `native`, median of rounds; ratios to serde_json. The A/A row is serde_json against itself: differences smaller than it are noise.\n\n");
        let mut cols = vec![
            "Workload".to_owned(),
            "Op".to_owned(),
            "serde_json (ns)".to_owned(),
            "A/A".to_owned(),
        ];
        let entries: Vec<&Entry> = ENTRIES.iter().skip(1).collect();
        cols.extend(entries.iter().map(|e| format!("`{}`", e.backend)));
        let refs: Vec<&str> = cols.iter().map(String::as_str).collect();
        out.push_str(&header(&refs));
        for w in payloads::ALL {
            for op in ["decode", "encode"] {
                let base = host.time.get(&Key::new("native", "serde_json", w, op), "ns");
                if base.is_none() {
                    continue;
                }
                let mut cells = vec![
                    (*w).to_owned(),
                    op.to_owned(),
                    num(base),
                    ratio(host.time.get(&Key::new("native", "serde_json#aa", w, op), "ns"), base),
                ];
                cells.extend(
                    entries
                        .iter()
                        .map(|e| ratio(host.time.get(&Key::new("native", e.backend, w, op), "ns"), base)),
                );
                out.push_str(&row(&cells));
            }
        }
        out.push('\n');
    }
}

fn provenance(out: &mut String, data: &Data) {
    out.push_str("### Provenance\n\n");
    out.push_str(&header(&["Source", "Host / arch", "Commit", "Recorded", "Notes"]));
    for (arch, table) in &data.callgrind {
        let p = &table.provenance;
        out.push_str(&row(&[
            "callgrind".into(),
            arch.clone(),
            format!("`{}`", p["git_commit"].as_str().unwrap_or("?").get(..8).unwrap_or("?")),
            p["timestamp"].as_str().unwrap_or("?").to_owned(),
            format!(
                "{}; cpu {}",
                p["valgrind"].as_str().unwrap_or("?"),
                p["cpu"].as_str().unwrap_or("?")
            ),
        ]));
    }
    for h in &data.hosts {
        let p = &h.provenance;
        if p.is_null() {
            continue;
        }
        out.push_str(&row(&[
            "host".into(),
            h.slug.clone(),
            format!("`{}`", p["git_commit"].as_str().unwrap_or("?").get(..8).unwrap_or("?")),
            p["timestamp"].as_str().unwrap_or("?").to_owned(),
            format!(
                "kernel {}, governor {}, boost {}; doctor: {}",
                p["kernel"].as_str().unwrap_or("?"),
                p["governor"].as_str().unwrap_or("?"),
                p["boost"].as_str().unwrap_or("?"),
                match h.doctor["failures"].as_array() {
                    Some(f) if f.is_empty() => "every build runs its claimed SIMD path".to_owned(),
                    Some(f) => format!("{} failures", f.len()),
                    None => "not run".to_owned(),
                }
            ),
        ]));
    }
    let _ = POINTS;
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::verdict::ENTRIES;
    use super::verdict::tests::arm_e2e_counts;
    use super::{depth_cell, end_to_end, limitations};
    use crate::report::data::{Data, Host, Key, Table};

    #[test]
    fn the_aarch64_end_to_end_table_reads_the_counted_build() {
        let e = ENTRIES.iter().find(|e| e.set == "sonic-rs").unwrap_or(&ENTRIES[1]);
        let mut out = String::new();
        end_to_end(&mut out, &arm_e2e_counts(e.backend));
        assert!(out.contains("**Counted, aarch64 native-counted**"), "{out}");
        // native-counted holds 300 and 200 cycles; native holds 100 and 50.
        assert!(out.contains("| 200 |"), "{out}");
        assert!(!out.contains("| 50 |"), "{out}");
    }

    fn host(slug: &str, cpu: &str, trust: &str, (rounds, seconds): (usize, u64)) -> Host {
        let mut time = Table::default();
        time.rows.insert(
            Key::new("native", "serde_json", "json-small", "decode"),
            json!({ "trust": trust, "rounds": vec![json!({}); rounds] }),
        );
        time.provenance = json!({ "loadavg": "41.20 38.00 35.10 3/900 1" });
        Host {
            slug: slug.into(),
            time,
            e2e_time: vec![json!({ "trust": trust, "reps": [{}, {}, {}, {}, {}], "seconds": seconds })],
            provenance: json!({ "cpu": cpu }),
            ..Host::default()
        }
    }

    #[test]
    fn states_missing_hosts_solo_class_and_reductions() {
        let data = Data {
            hosts: vec![host("x86_64-linux-amd-x", "AMD X", "solo", (2, 10))],
            fuzz: vec![json!({ "seconds": 1800 })],
            ..Data::default()
        };
        let notes = limitations(&data).join("\n");
        assert!(notes.contains("No Intel host: every host-measured x86-64 number is from AMD X."));
        assert!(notes.contains("No macOS host"));
        assert!(notes.contains("`solo` class"));
        assert!(notes.contains("load average 41.20 38.00 35.10"));
        assert!(notes.contains("timed sampling 2 rounds (reduced from the default 3)"));
        assert!(notes.contains("end to end 5 reps of 10 s (reduced from the default 30) per load phase"));
        assert!(notes.contains("Fuzzing ran 1800 s per target"));
        assert!(!notes.contains("1800 s (reduced"));
    }

    #[test]
    fn retires_notes_the_results_answer() {
        let data = Data {
            hosts: vec![
                host("x86_64-linux-intel-y", "Intel Y", "canonical", (3, 30)),
                host("aarch64-macos-apple-m", "Apple M", "canonical", (3, 30)),
            ],
            ..Data::default()
        };
        let notes = limitations(&data).join("\n");
        assert!(!notes.contains("No Intel host"));
        assert!(!notes.contains("No macOS host"));
        assert!(!notes.contains("solo"));
        assert!(!notes.contains("reduced"));
    }

    #[test]
    fn depth_limits_say_what_came_after_the_deepest_accepted() {
        let limits = json!({
            "array/value/2MiB": { "max_accepted": 1025, "first_rejected": null, "first_crashed": 10000 },
            "object/value/2MiB": { "max_accepted": 1025, "first_rejected": null, "first_crashed": 10000 },
            "array/ignored/2MiB": { "max_accepted": 127, "first_rejected": 128, "first_crashed": null },
            "object/ignored/2MiB": { "max_accepted": 1000000, "first_rejected": null, "first_crashed": null },
        });
        assert_eq!(depth_cell(&limits, "value", "2MiB"), "≤1025, crashes at 10000");
        assert_eq!(
            depth_cell(&limits, "ignored", "2MiB"),
            "array ≤127, rejects 128; object all, to 1000000"
        );
        assert_eq!(depth_cell(&limits, "value", "8MiB"), "—");
    }

    #[test]
    fn states_a_missing_x86_64_host() {
        let data = Data {
            hosts: vec![host("aarch64-macos-apple-m", "Apple M", "canonical", (3, 30))],
            ..Data::default()
        };
        let notes = limitations(&data).join("\n");
        assert!(notes.contains("No x86-64 host is recorded, so no Intel host either."));
        assert!(!notes.contains("No Intel host:"));
    }
}
