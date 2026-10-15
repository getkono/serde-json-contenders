//! The frontier, the verdict, and the generated report.
//!
//! Everything under `report/` is a pure function of `results/`, written by
//! `cargo xtask report`; `--check` fails when any of it differs from what the
//! results generate, so no number can drift from its evidence. The
//! repository README holds the method and the commentary; every number the
//! commentary quotes is a link into `report/README.md`, and `--check`
//! verifies each one against the row it names.

mod data;
mod export;
mod figures;
mod notes;
mod render;
mod verdict;

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::util::root;

/// `xtask report [--check] [--allow-stale]`.
///
/// `--allow-stale` renders results not measured on the committed code, for
/// work in progress; the report says so on its first line, and `--check`
/// never accepts it.
pub fn report(args: &[String]) -> Result<()> {
    let check = args.iter().any(|a| a == "--check");
    let allow_stale = args.iter().any(|a| a == "--allow-stale") && !check;
    let data = data::Data::load()?;
    let stale = data::stale(&data.sources, crate::provenance::fingerprint().as_deref());
    if !stale.is_empty() && !allow_stale {
        bail!(
            "results not measured on the committed code (rerun the stage, or pass --allow-stale for a draft):\n{}",
            stale.join("\n")
        );
    }
    let verdicts = verdict::judge(&data);
    let (figures, unfigured) = figures::figures(&data, &verdict::conforming(&data));
    let mut problems = data::incomplete(&data);
    problems.extend(notes::check(&data));
    problems.extend(unfigured);
    if !problems.is_empty() && !allow_stale {
        bail!("results are incomplete:\n{}", problems.join("\n"));
    }
    let mut stale_notes = stale;
    stale_notes.extend(problems);
    let exported = export::exports(&data);
    let body = render::render(&data, &verdicts, &figures, &exported, &stale_notes);
    let mut files: Vec<(String, String)> = vec![
        ("README.md".into(), body.clone()),
        (
            "verdict.json".into(),
            format!("{}\n", serde_json::to_string_pretty(&verdict::to_json(&verdicts))?),
        ),
    ];
    files.extend(figures.iter().map(|f| (format!("figures/{}", f.file), f.svg.clone())));
    files.extend(exported.iter().map(|e| (format!("data/{}", e.name), e.csv.clone())));
    let dir = root().join("report");
    let readme = std::fs::read_to_string(root().join("README.md")).context("reading README.md")?;
    let mut claims = check_claims(&readme, &body);
    claims.extend(bare_figures(&readme));
    claims.extend(dead_links(&readme, &files));
    if check {
        let mut differ: Vec<String> = files
            .iter()
            .filter(|(name, text)| std::fs::read_to_string(dir.join(name)).ok().as_deref() != Some(text))
            .map(|(name, _)| format!("report/{name}"))
            .collect();
        differ.extend(extra_files(&dir, &files)?);
        if !differ.is_empty() {
            bail!("report is stale (run `cargo xtask report`): {}", differ.join(", "));
        }
        if !claims.is_empty() {
            bail!("README claims the report does not bear out:\n{}", claims.join("\n"));
        }
        eprintln!(
            "report: report/ is current and every README claim matches it ({} files)",
            files.len()
        );
        return Ok(());
    }
    for sub in ["figures", "data"] {
        if dir.join(sub).exists() {
            std::fs::remove_dir_all(dir.join(sub))?;
        }
    }
    for (name, text) in &files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    eprintln!("report: wrote {} files under report/", files.len());
    for c in &claims {
        eprintln!("report: README claim not borne out: {c}");
    }
    Ok(())
}

/// Files under `dir` that the report does not generate.
fn extra_files(dir: &Path, files: &[(String, String)]) -> Result<Vec<String>> {
    let mut extra = Vec::new();
    if !dir.exists() {
        return Ok(extra);
    }
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
        if !files.iter().any(|(name, _)| *name == rel) {
            extra.push(format!("report/{rel} (not generated)"));
        }
    }
    Ok(extra)
}

/// Every README link into the report that quotes a figure, written
/// `[0.84×](report/README.md#anchor "row words | column")`, checked against
/// the generated report. Under `anchor`, exactly one table row must hold
/// every row word in its cells, and the cell under the named column must be
/// the quoted text, or hold it as a whole figure (not as part of a longer
/// number). Without a column, some cell of that row must. A link whose text
/// holds a digit and that names no row is itself a failure: every quoted
/// figure says where it comes from. Returns what fails.
fn check_claims(readme: &str, report: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut rest = readme;
    while let Some(start) = rest.find("](report/README.md#") {
        let text_start = rest[..start].rfind('[').map_or(start, |i| i + 1);
        let quoted = &rest[text_start..start];
        let after = &rest[start + "](report/README.md#".len()..];
        let (anchor, title, consumed) = match after.find([' ', ')']) {
            Some(i) if after[i..].starts_with(" \"") => {
                let t = &after[i + 2..];
                let Some(close) = t.find("\")") else {
                    failures.push(format!("[{quoted}]: unterminated link title"));
                    break;
                };
                (&after[..i], &t[..close], i + 2 + close + 2)
            }
            Some(i) => (&after[..i], "", i + 1),
            None => break,
        };
        rest = &after[consumed..];
        if title.is_empty() {
            if quoted.chars().any(|c| c.is_ascii_digit()) {
                failures.push(format!("[{quoted}](#{anchor}): a quoted figure must name its row"));
            }
            continue;
        }
        let Some(section) = anchored(report, anchor) else {
            failures.push(format!("[{quoted}] links to #{anchor}, which the report does not have"));
            continue;
        };
        if let Err(e) = holds(section, title, quoted) {
            failures.push(format!("[{quoted}](#{anchor} \"{title}\"): {e}"));
        }
    }
    failures
}

/// Whether a table under `section` has exactly one row matching `title`'s
/// row words (each a whole cell, or a whole word of one) whose named column
/// (or any cell) holds `quoted`.
fn holds(section: &str, title: &str, quoted: &str) -> Result<(), String> {
    let (words, column) = match title.split_once(" | ") {
        Some((w, c)) => (w, Some(c.trim())),
        None => (title, None),
    };
    let words: Vec<&str> = words.split_whitespace().collect();
    let cells = |line: &str| -> Vec<String> {
        line.trim()
            .trim_matches('|')
            .split(" | ")
            .map(|c| c.trim().to_owned())
            .collect()
    };
    let mut header: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    for line in section.lines() {
        if !line.starts_with('|') {
            header.clear();
            continue;
        }
        if line.starts_with("| ---") {
            continue;
        }
        if header.is_empty() {
            header = cells(line);
            continue;
        }
        let row = cells(line);
        if words
            .iter()
            .all(|w| row.iter().any(|c| c == w || c.split_whitespace().any(|t| t == *w)))
        {
            rows.push((header.clone(), row));
        }
    }
    let [(header, row)] = rows.as_slice() else {
        return Err(format!("{} rows match \"{title}\", not one", rows.len()));
    };
    let candidates: Vec<&String> = match column {
        Some(c) => {
            let i = header
                .iter()
                .position(|h| h == c)
                .ok_or_else(|| format!("no column {c}"))?;
            row.get(i).into_iter().collect()
        }
        None => row.iter().collect(),
    };
    if candidates.iter().any(|c| whole(c, quoted)) {
        Ok(())
    } else {
        Err(format!("the row holds {candidates:?}, not {quoted}"))
    }
}

/// Whether `cell` holds `quoted` as a whole figure: equal, or present with
/// no digit, point, comma or sign directly around it.
fn whole(cell: &str, quoted: &str) -> bool {
    let joins = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '+' | '-' | '−'));
    cell.match_indices(quoted)
        .any(|(i, _)| !joins(cell[..i].chars().next_back()) && !joins(cell[i + quoted.len()..].chars().next()))
}

/// Figures written in the README's Findings without a link to the report row
/// they come from. A number there is a claim, and every claim names its
/// evidence. Inline code (`nested-129`) and a rule's number ("rule 4") are
/// names, not figures.
fn bare_figures(readme: &str) -> Vec<String> {
    let Some(start) = readme
        .find("\n## Findings")
        .map(|i| i + 1)
        .or_else(|| readme.starts_with("## Findings").then_some(0))
    else {
        return Vec::new();
    };
    let body = &readme[start..];
    let end = body[3..].find("\n## ").map_or(body.len(), |i| i + 3);
    let mut text = String::new();
    let mut rest = &body[..end];
    // Drop link texts with their targets, and inline code.
    while let Some(i) = rest.find(['[', '`']) {
        text.push_str(&rest[..i]);
        let close = if rest[i..].starts_with('`') {
            rest[i + 1..].find('`').map(|j| i + 1 + j + 1)
        } else {
            rest[i..]
                .find("](")
                .and_then(|j| rest[i + j..].find(')').map(|k| i + j + k + 1))
        };
        let Some(close) = close else {
            rest = &rest[i + 1..];
            continue;
        };
        text.push(' ');
        rest = &rest[close..];
    }
    text.push_str(rest);
    let mut failures = Vec::new();
    let mut previous = "";
    for word in text.split_whitespace() {
        let token = word.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '.' | '(' | ')' | '"' | '*'));
        let figure = token.chars().any(|c| c.is_ascii_digit())
            && token
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '%' | '×' | '+' | '-' | '−'));
        if figure && !matches!(previous.to_lowercase().as_str(), "rule" | "rules") {
            failures.push(format!(
                "Findings quotes {token} without a link to the report row it comes from"
            ));
        }
        previous = token;
    }
    failures
}

/// README links into `report/` that point at no generated file.
fn dead_links(readme: &str, files: &[(String, String)]) -> Vec<String> {
    readme
        .match_indices("](report/")
        .filter_map(|(i, _)| {
            let target = &readme[i + 2..];
            let end = target.find([')', ' ', '#']).unwrap_or(target.len());
            let file = target[..end].trim_start_matches("report/");
            (!files.iter().any(|(name, _)| name == file))
                .then(|| format!("README links to report/{file}, which the report does not generate"))
        })
        .collect()
}

/// The report text from `anchor`'s marker to the next anchor marker.
fn anchored<'a>(report: &'a str, anchor: &str) -> Option<&'a str> {
    let marker = format!("<a id=\"{anchor}\"></a>");
    let start = report.find(&marker)? + marker.len();
    let end = report[start..].find("<a id=\"").map_or(report.len(), |i| start + i);
    Some(&report[start..end])
}

#[cfg(test)]
mod tests {
    use super::check_claims;

    const REPORT: &str = "<a id=\"kynos-decode\"></a>\n\n## x\n\n| Workload | Point | serde_json | `sonic-rs` | `jiter` |\n| --- | --- | --- | --- | --- |\n| json-small | native (cycles) | 1633 | 0.84× | 10.84× |\n| json-large | native (cycles) | 2.1 M | 0.80× | 0.84× |\n\n<a id=\"heap\"></a>\n\n| Entry point | Workload | Decode allocations |\n| --- | --- | --- |\n| `simd-json` | json-small | 11 |\n";

    #[test]
    fn a_claim_the_named_cell_holds_passes() {
        let readme = "sonic-rs takes [0.84×](report/README.md#kynos-decode \"json-small native (cycles) | `sonic-rs`\") of the cycles.";
        assert_eq!(check_claims(readme, REPORT), Vec::<String>::new());
    }

    #[test]
    fn an_edited_figure_fails() {
        let readme = "[0.80×](report/README.md#kynos-decode \"json-small native (cycles) | `sonic-rs`\")";
        assert_eq!(check_claims(readme, REPORT).len(), 1);
    }

    /// The figure is in the row, but under another backend.
    #[test]
    fn a_figure_from_another_column_fails() {
        let readme = "[0.84×](report/README.md#kynos-decode \"json-large native (cycles) | `sonic-rs`\")";
        assert_eq!(check_claims(readme, REPORT).len(), 1);
    }

    /// "0.84×" is not the "10.84×" in jiter's cell, and "11" is not "110".
    #[test]
    fn part_of_a_longer_number_is_not_the_figure() {
        let readme = "[0.84×](report/README.md#kynos-decode \"json-small native (cycles) | `jiter`\")";
        assert_eq!(check_claims(readme, REPORT).len(), 1);
    }

    #[test]
    fn a_figure_from_another_section_fails() {
        let readme = "[11](report/README.md#kynos-decode \"simd-json json-small\")";
        assert_eq!(check_claims(readme, REPORT).len(), 1);
        let readme = "[11](report/README.md#heap \"`simd-json` json-small | Decode allocations\")";
        assert!(check_claims(readme, REPORT).is_empty());
    }

    #[test]
    fn a_row_two_lines_match_is_ambiguous() {
        let readme = "[0.84×](report/README.md#kynos-decode \"native (cycles)\")";
        assert_eq!(check_claims(readme, REPORT).len(), 1);
    }

    #[test]
    fn a_figure_in_findings_without_a_link_fails() {
        use super::bare_figures;
        let ok = "## Findings\n\nsonic-rs takes [0.84×](report/README.md#x \"y\") under rule 1 on `nested-129`.\n\n## Licence\n\n2026";
        assert!(bare_figures(ok).is_empty(), "{:?}", bare_figures(ok));
        let bad = "## Findings\n\nsonic-rs takes 0.84× of the cycles, 16 % less.\n\n## Licence\n";
        assert_eq!(bare_figures(bad).len(), 2, "{:?}", bare_figures(bad));
    }

    #[test]
    fn a_quoted_figure_without_a_row_fails_and_a_plain_link_does_not() {
        assert_eq!(check_claims("[0.84×](report/README.md#kynos-decode)", REPORT).len(), 1);
        assert!(check_claims("see [the heap table](report/README.md#heap)", REPORT).is_empty());
    }
}
