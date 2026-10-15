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

/// One Markdown link: `[text](target "title")`.
#[derive(Debug, Clone, Copy)]
struct Link<'a> {
    text: &'a str,
    target: &'a str,
    title: Option<&'a str>,
    /// Byte range of the whole link in the scanned text.
    start: usize,
    end: usize,
}

/// Every link in `s`, in order. A `[` that does not open a link ("[sic]")
/// is text, and hides nothing after it.
fn links(s: &str) -> Vec<Link<'_>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = s[from..].find('[').map(|i| from + i) {
        from = open + 1;
        let Some(close) = s[open + 1..].find([']', '[']).map(|i| open + 1 + i) else {
            break;
        };
        if !s[close..].starts_with("](") {
            continue;
        }
        let after = close + 2;
        let Some(stop) = s[after..].find([' ', ')']).map(|i| after + i) else {
            continue;
        };
        let target = &s[after..stop];
        let (title, end) = if s[stop..].starts_with(" \"") {
            match s[stop + 2..].find("\")") {
                Some(t) => (Some(&s[stop + 2..stop + 2 + t]), stop + 2 + t + 2),
                None => continue,
            }
        } else if s[stop..].starts_with(')') {
            (None, stop + 1)
        } else {
            continue;
        };
        out.push(Link {
            text: &s[open + 1..close],
            target,
            title,
            start: open,
            end,
        });
        from = end;
    }
    out
}

/// Every README link into the report, checked against the generated report.
/// The anchor must exist. A link whose text holds a digit quotes a figure,
/// and must be written `[0.84×](report/README.md#anchor "row words |
/// column")`: under `anchor`, exactly one table row must hold every row word
/// in its cells, and the cell under the named column must be the quoted
/// text, or hold it as a whole figure (not as part of a longer number).
/// Returns what fails.
fn check_claims(readme: &str, report: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for link in links(readme) {
        let Some(rest) = link.target.strip_prefix("report/README.md") else {
            continue;
        };
        let quoted = link.text;
        let figure = quoted.chars().any(char::is_numeric);
        let Some(anchor) = rest.strip_prefix('#') else {
            if figure {
                failures.push(format!("[{quoted}]: a quoted figure must link to its section's anchor"));
            }
            continue;
        };
        let Some(section) = anchored(report, anchor) else {
            failures.push(format!("[{quoted}] links to #{anchor}, which the report does not have"));
            continue;
        };
        if !figure {
            continue;
        }
        let Some(title) = link.title.filter(|t| t.contains(" | ")) else {
            failures.push(format!(
                "[{quoted}](#{anchor}): a quoted figure must name its row and column (\"row words | column\")"
            ));
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
/// no digit, point, comma or sign directly before it, and nothing after it
/// that would make it a longer number or give it a unit it does not quote
/// ("2.10" is not the "2.10 M" in a cell).
fn whole(cell: &str, quoted: &str) -> bool {
    let joins = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '+' | '-' | '−'));
    // A quote with a unit of its own ("0.80×", "16 %", "2.1 M") says what it
    // is; only a bare number could be mistaken for one with another unit.
    let bare = quoted
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '+' | '-' | '−' | ' '));
    // A unit is a word of its own after the number ("2.10 M", "16 %"), not
    // any word ("4 of 4").
    let unit = |rest: &str| {
        if !bare {
            // "2.93 M" is not the "2.93 MB" in a cell.
            return quoted.ends_with(char::is_alphabetic) && rest.starts_with(char::is_alphanumeric);
        }
        let next = rest.trim_start_matches(' ');
        let word = next
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ')' | '('))
            .next()
            .unwrap_or("");
        rest.starts_with(['%', '×']) || UNITS.contains(&word)
    };
    cell.match_indices(quoted).any(|(i, _)| {
        let rest = &cell[i + quoted.len()..];
        !joins(cell[..i].chars().next_back()) && !joins(rest.chars().next()) && !unit(rest)
    })
}

/// The units a figure is written with, as words of their own.
const UNITS: &[&str] = &[
    "%",
    "×",
    "x",
    "k",
    "K",
    "M",
    "G",
    "B",
    "KiB",
    "MiB",
    "GiB",
    "kB",
    "MB",
    "GB",
    "s",
    "ms",
    "µs",
    "ns",
    "rps",
    "cycles",
    "instructions",
    "allocations",
    "bytes",
];

/// Whether a word of Findings names something rather than quoting a figure:
/// a version (`1.0.154`), a date (`2026-10-15`) or a width (`64-bit`).
fn is_name_number(token: &str) -> bool {
    let digits_dots = token
        .split('.')
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let date = token.len() == 10
        && token
            .char_indices()
            .all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
    (digits_dots && token.matches('.').count() >= 2)
        || date
        || token
            .strip_suffix("-bit")
            .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
}

/// Whether a word of Findings is a figure. In code, a name (`nested-129`)
/// is not, but a number (`16`, `~0.8`) is. In prose, any word holding a
/// digit that does not begin as a name does (with a letter), as a footnote
/// marker (`^1`) or as an issue (`#123`) is. Anywhere, a figure joined to a
/// name by anything but a hyphen, underscore, point or `#` (`p99=12`,
/// `cycles(1633)`, `json-small@0.84×`) is one; `x86-64` and `nested-129`
/// are names. A name with a digit
/// in it that does not begin with a letter (`7800X3D`, `99th`) reads as a
/// figure too: Findings refers to such things in words, or links them.
fn is_figure(token: &str, code: bool) -> bool {
    if is_name_number(token) {
        return false;
    }
    if token
        .split(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '_' | '.' | '#' | '^' | '\'' | '’'))
        .skip(1)
        .any(|rest| {
            rest.trim_start_matches(['~', '+', '-', '−'])
                .starts_with(|c: char| c.is_numeric())
        })
    {
        return true;
    }
    let first = token.chars().next();
    if code {
        token
            .trim_start_matches(['~', '≈', '±', '+', '-', '−'])
            .chars()
            .next()
            .is_some_and(char::is_numeric)
    } else {
        token.chars().any(char::is_numeric) && !first.is_some_and(|c| c.is_alphabetic() || matches!(c, '^' | '#'))
    }
}

/// Figures written in the README's Findings without a link to the report row
/// they come from. A number there is a claim, and every claim names its
/// evidence. See [`is_figure`] for what a figure is; a list item's number
/// ("1." at the start of a line) and a rule's number ("rule 1", "rules 1,
/// 2 and 4", "rules 1–4", "rule 1's") are not figures. A link elsewhere may
/// name things ("sonic-rs#123"), but not quote figures.
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
    let body = &body[..end];
    let mut failures = Vec::new();
    // Links become a space.
    let mut text = String::new();
    let mut at = 0;
    for link in links(body) {
        if !link.target.starts_with("report/README.md#") && link.text.split_whitespace().any(|w| is_figure(w, false)) {
            failures.push(format!(
                "Findings quotes [{}] from {}, not from a report row",
                link.text, link.target
            ));
        }
        text.push_str(&body[at..link.start]);
        text.push(' ');
        at = link.end;
    }
    text.push_str(&body[at..]);
    if text.contains("<a ") || text.contains("<span") || text.contains("<b>") {
        failures.push("Findings holds HTML; quote figures as Markdown links".to_owned());
    }
    // Words: (text, in code, first on its line). A code span is one word.
    // In prose a dash or a slash splits words, so "fewer—16 %" and "1/3"
    // yield their numbers.
    let mut words: Vec<(String, bool, bool)> = Vec::new();
    let mut code = false;
    let mut word = String::new();
    let mut line_start = true;
    let mut word_line_start = true;
    for c in text.chars() {
        let split = c == '`' || c.is_whitespace() || (!code && matches!(c, '—' | '–' | '/'));
        if split {
            if !word.is_empty() {
                words.push((std::mem::take(&mut word), code, word_line_start));
                line_start = false;
            }
            if c == '`' {
                code = !code;
            }
            if c == '\n' {
                line_start = true;
            }
        } else {
            if word.is_empty() {
                word_line_start = line_start;
            }
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push((word, code, word_line_start));
    }
    if code {
        failures.push("Findings has an unmatched backtick, which hides the text after it".to_owned());
    }
    let trim = |w: &str| -> String {
        w.trim_matches(|c: char| {
            matches!(
                c,
                ',' | ';' | ':' | '.' | '(' | ')' | '"' | '*' | '_' | '[' | ']' | '!' | '?' | '\''
            )
        })
        .to_owned()
    };
    let mut rules = false;
    for (i, (word, code, first)) in words.iter().enumerate() {
        let (word, code, first) = (word.as_str(), *code, *first);
        let trimmed = trim(word);
        let trimmed = trimmed.as_str();
        // "rule 1, 4 % faster": a number with a unit after it is a figure,
        // whatever came before it.
        let unit_follows = words
            .get(i + 1)
            .is_some_and(|(next, _, _)| next.starts_with(['%', '×']) || UNITS.contains(&trim(next).as_str()));
        let possessive = trimmed.strip_suffix("'s").or_else(|| trimmed.strip_suffix("’s"));
        let token = possessive.unwrap_or(trimmed);
        let lower = token.to_lowercase();
        let list_item = first && !code && word.ends_with('.') && token.chars().all(|c| c.is_ascii_digit());
        let rule = !code && rules && !unit_follows && matches!(token, "1" | "2" | "3" | "4");
        if is_figure(token, code) && !rule && !list_item {
            failures.push(format!(
                "Findings quotes {token} without a link to the report row it comes from"
            ));
        }
        // A list of rule numbers runs on through "and", "or", "to" and
        // commas, and ends at any other word or at a colon or possessive.
        rules = !code
            && (matches!(lower.as_str(), "rule" | "rules")
                || (rules
                    && (rule || matches!(lower.as_str(), "and" | "or" | "to"))
                    && possessive.is_none()
                    && !word.ends_with([':', '.', ';'])));
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
    fn findings_figures_in_any_form_need_a_link() {
        use super::bare_figures;
        let ok = "## Findings\n\nUnder rules 1 and 4, at `v3` and `x86-64`, [sic] sonic-rs takes [0.84×](report/README.md#x \"y | z\").\n";
        assert!(bare_figures(ok).is_empty(), "{:?}", bare_figures(ok));
        for bad in [
            "0.84x",
            "~16",
            "1.2k",
            "10–20",
            "≈0.8×",
            "1/3",
            "rule 1 at 16 %",
            "`16 %`",
            "_0.84×_",
            "fewer—16 %",
            "rule 4: 30 %",
            "rule 4's 10 %",
            "rule 1 and 16 %",
            "<a href=\"x\">0.84×</a>",
            "１６ %",
            "`json-small: 0.84×`",
            "`~16 %`",
            "passes rule 1, 4 % faster",
            "rule 1 and 3 % of requests",
            "\n```\nsonic-rs 0.84x 16 %\n```\n",
            "an `unmatched backtick 16 %",
            "p99=12 ms",
            "`cycles=1633`",
            "passes rule 1, 4 ms faster",
            "rule 1 and 2 M cycles fewer",
            "Zen 4 % faster",
            "Only 3 entries pass rule 1.",
            "Across 12 workloads it never loses.",
            "p99<12 ms",
            "RFC 8259",
            "sonic-rs(0.84×)",
            "json-small@0.84×",
            "speedup×1.19",
        ] {
            let text = format!("## Findings\n\n[sic] it is {bad} faster.\n");
            assert!(!bare_figures(&text).is_empty(), "{bad} passed");
        }
        let elsewhere = "## Findings\n\nIt is [0.84×](https://example.com) faster.\n";
        assert_eq!(bare_figures(elsewhere).len(), 1);
    }

    #[test]
    fn a_figure_without_its_unit_is_not_the_cell() {
        assert!(!super::whole("2.10 M", "2.10"));
        assert!(super::whole("2.10 M", "2.10 M"));
        assert!(super::whole("0.84× (0.90× at v3)", "0.84×"));
        assert!(super::whole(
            "✅ json-small decode CPU 0.80× at native (cycles)",
            "0.80×"
        ));
        assert!(super::whole("fails 4 of 4 conformance runs", "4"));
        assert!(super::whole("serde_json's 4,584 for twitter-like", "4,584"));
        assert!(!super::whole("2.93 MB", "2.93 M"));
        let refs = "## Findings\n\nSee [#123](https://github.com/x/y/issues/123), with serde_json 1.0.154 on 2026-10-15, a 64-bit build.\n";
        assert!(super::bare_figures(refs).is_empty(), "{:?}", super::bare_figures(refs));
        let ok = "## Findings\n\nUnder rules 1, 2 and 4 it holds.\n\n1. First, rule 1's test and rules 1–4 and rules 1 to 4.\n2. On `Zen`, see [sonic-rs#123](https://github.com/x) and [^1].\n";
        assert!(super::bare_figures(ok).is_empty(), "{:?}", super::bare_figures(ok));
    }

    #[test]
    fn a_figure_must_name_its_column_and_every_anchor_must_exist() {
        let no_column = "[10.84×](report/README.md#kynos-decode \"json-small native (cycles)\")";
        assert_eq!(check_claims(no_column, REPORT).len(), 1);
        assert_eq!(check_claims("[0.84×](report/README.md)", REPORT).len(), 1);
        assert_eq!(check_claims("see [this](report/README.md#nowhere)", REPORT).len(), 1);
    }

    #[test]
    fn a_quoted_figure_without_a_row_fails_and_a_plain_link_does_not() {
        assert_eq!(check_claims("[0.84×](report/README.md#kynos-decode)", REPORT).len(), 1);
        assert!(check_claims("see [the heap table](report/README.md#heap)", REPORT).is_empty());
    }
}
