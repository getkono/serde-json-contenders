//! The capabilities `results/notes.toml` claims, with their figures filled in
//! from the results.
//!
//! A note's prose is hand-written; its numbers are not. Each `{name}` in a
//! `merit` names an entry of that backend's `figures` table, a query into a
//! result file, and the report substitutes what the file holds. A figure the
//! results cannot answer fails the report instead of printing a stale number.

use super::data::{Data, Key};

/// The merit text for `backend`, figures filled in; `None` without a note.
pub fn merit(data: &Data, backend: &str) -> Option<String> {
    let note = data.notes.get(backend)?;
    let text = note.get("merit")?.as_str()?;
    Some(fill(data, text, note.get("figures").and_then(toml::Value::as_table)).unwrap_or_else(|e| e))
}

/// Every note whose figures the results cannot answer.
pub fn check(data: &Data) -> Vec<String> {
    data.notes
        .iter()
        .filter_map(|(backend, note)| {
            let text = note.get("merit")?.as_str()?;
            fill(data, text, note.get("figures").and_then(toml::Value::as_table))
                .err()
                .map(|e| format!("notes.toml [{backend}]: {e}"))
        })
        .collect()
}

/// `text` with each `{name}` replaced by its figure.
fn fill(data: &Data, text: &str, figures: Option<&toml::Table>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            return Err(format!("unclosed {{ in {text:?}"));
        };
        let name = &rest[open + 1..open + close];
        let query = figures
            .and_then(|f| f.get(name))
            .and_then(toml::Value::as_table)
            .ok_or_else(|| format!("no figure named {name}"))?;
        out.push_str(&figure(data, query).ok_or_else(|| format!("figure {name} is not in the results"))?);
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// One figure: `table` is `alloc` (a row of the host's allocation counts,
/// keyed like every per-operation row; `variant` defaults to `native`) or
/// `footprint` (a set's row). `format` is `count` (default) or `bytes`.
fn figure(data: &Data, q: &toml::Table) -> Option<String> {
    let s = |k: &str| q.get(k).and_then(toml::Value::as_str);
    let field = s("field")?;
    let value = match s("table")? {
        "alloc" => {
            let key = Key {
                variant: s("variant").unwrap_or("native").to_owned(),
                backend: s("backend")?.to_owned(),
                workload: s("workload")?.to_owned(),
                op: s("op")?.to_owned(),
                form: s("form").unwrap_or("owned").to_owned(),
                arrival: s("arrival").unwrap_or("shared").to_owned(),
            };
            data.primary()?.alloc.get(&key, field)?
        }
        "footprint" => {
            let set = s("set")?;
            data.footprint
                .iter()
                .find(|r| r["set"] == set)
                .and_then(|r| r[field].as_f64())?
        }
        _ => return None,
    };
    Some(match s("format").unwrap_or("count") {
        "bytes" => bytes(value),
        _ => count(value),
    })
}

/// A count with thousands separators, rounded to a whole number.
fn count(v: f64) -> String {
    let digits = format!("{:.0}", v.abs());
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if v < 0.0 { format!("-{out}") } else { out }
}

/// Decimal bytes, to three significant figures.
fn bytes(v: f64) -> String {
    match v {
        v if v >= 1e6 => format!("{:.2} MB", v / 1e6),
        v if v >= 1e3 => format!("{:.0} kB", v / 1e3),
        v => format!("{v:.0} B"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{check, count, merit};
    use crate::report::data::{Data, Key};

    fn data(notes: &str) -> Data {
        let mut data = Data {
            notes: notes.parse().unwrap(),
            footprint: vec![json!({ "set": "hifijson", "unsafe_blocks": 0 })],
            ..Data::default()
        };
        data.host.slug = "x86_64-linux-amd-x".into();
        let mut key = Key::new("native", "simd-json", "twitter-like", "decode");
        key.form = "borrowed".into();
        key.arrival = "unique".into();
        data.host
            .alloc
            .rows
            .insert(key, json!({ "allocations": 1175.0, "bytes": 2_930_000.0 }));
        data
    }

    #[test]
    fn fills_figures_from_the_results() {
        let d = data(
            r#"
[simd-json]
merit = "{n} allocations, {b}; {u} unsafe blocks."
figures.n = { table = "alloc", backend = "simd-json", workload = "twitter-like", op = "decode", form = "borrowed", arrival = "unique", field = "allocations" }
figures.b = { table = "alloc", backend = "simd-json", workload = "twitter-like", op = "decode", form = "borrowed", arrival = "unique", field = "bytes", format = "bytes" }
figures.u = { table = "footprint", set = "hifijson", field = "unsafe_blocks" }
"#,
        );
        assert_eq!(
            merit(&d, "simd-json").as_deref(),
            Some("1,175 allocations, 2.93 MB; 0 unsafe blocks.")
        );
        assert!(check(&d).is_empty());
    }

    #[test]
    fn a_figure_the_results_lack_fails_the_check() {
        let d = data(
            r#"
[simd-json]
merit = "{n} allocations"
figures.n = { table = "alloc", backend = "simd-json", workload = "citm-like", op = "decode", field = "allocations" }
"#,
        );
        assert_eq!(check(&d), ["notes.toml [simd-json]: figure n is not in the results"]);
    }

    #[test]
    fn counts_have_separators() {
        assert_eq!(count(4584.0), "4,584");
        assert_eq!(count(123.4), "123");
        assert_eq!(count(1_234_567.0), "1,234,567");
    }
}
