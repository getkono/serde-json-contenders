//! The frontier, the verdict, and the generated report.
//!
//! Everything under `report/` is a pure function of `results/`, written by
//! `cargo xtask report`; `--check` fails when any of it differs from what the
//! results generate, so no number can drift from its evidence. The
//! repository README holds the method and the commentary; every number the
//! commentary quotes is a link into `report/README.md`, and `--check`
//! verifies each one against the row it names.

mod data;
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
    let mut problems = data::incomplete(&data);
    problems.extend(notes::check(&data));
    if !problems.is_empty() && !allow_stale {
        bail!("results are incomplete:\n{}", problems.join("\n"));
    }
    let verdicts = verdict::judge(&data);
    let figures = figures::figures(&data, &verdict::conforming(&data));
    let mut stale_notes = stale;
    stale_notes.extend(problems);
    let body = render::render(&data, &verdicts, &figures, &stale_notes);
    let mut files: Vec<(String, String)> = vec![
        ("README.md".into(), body.clone()),
        (
            "verdict.json".into(),
            format!("{}\n", serde_json::to_string_pretty(&verdict::to_json(&verdicts))?),
        ),
    ];
    files.extend(figures.iter().map(|f| (format!("figures/{}", f.file), f.svg.clone())));
    let dir = root().join("report");
    let readme = std::fs::read_to_string(root().join("README.md")).context("reading README.md")?;
    let claims = check_claims(&readme, &body);
    if check {
        let mut differ: Vec<String> = files
            .iter()
            .filter(|(name, text)| std::fs::read_to_string(dir.join(name)).ok().as_deref() != Some(text))
            .map(|(name, _)| format!("report/{name}"))
            .collect();
        differ.extend(extra_files(&dir, &files)?);
        if !differ.is_empty() {
            bail!(
                "report is stale (run `cargo xtask report`): {}",
                differ.join(", ")
            );
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
    if dir.join("figures").exists() {
        std::fs::remove_dir_all(dir.join("figures"))?;
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
        let rel = entry
            .path()
            .strip_prefix(dir)?
            .to_string_lossy()
            .replace('\\', "/");
        if !files.iter().any(|(name, _)| *name == rel) {
            extra.push(format!("report/{rel} (not generated)"));
        }
    }
    Ok(extra)
}

/// Every README link into the report that quotes a figure, written
/// `[0.84×](report/README.md#anchor "row words")`, checked against the
/// generated report: under `anchor`, some line holds every word of the title
/// and the quoted text. Returns what fails.
fn check_claims(readme: &str, report: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut rest = readme;
    while let Some(start) = rest.find("](report/README.md#") {
        let text_start = rest[..start].rfind('[').map_or(start, |i| i + 1);
        let quoted = &rest[text_start..start];
        let after = &rest[start + "](report/README.md#".len()..];
        let Some(end) = after.find(')') else { break };
        let target = &after[..end];
        let (anchor, title) = match target.split_once(" \"") {
            Some((a, t)) => (a, t.trim_end_matches('"')),
            None => (target, ""),
        };
        rest = &after[end..];
        if title.is_empty() {
            continue;
        }
        let Some(section) = anchored(report, anchor) else {
            failures.push(format!("[{quoted}] links to #{anchor}, which the report does not have"));
            continue;
        };
        let words: Vec<&str> = title.split_whitespace().collect();
        let found = section
            .lines()
            .any(|l| words.iter().all(|w| l.contains(w)) && l.contains(quoted));
        if !found {
            failures.push(format!(
                "[{quoted}](#{anchor} \"{title}\"): no line under #{anchor} has \"{title}\" and \"{quoted}\""
            ));
        }
    }
    failures
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

    const REPORT: &str = "<a id=\"kynos-decode\"></a>\n\n## x\n\n| json-small | native (cycles) | 1633 | 1.01× | 0.84× |\n\n<a id=\"heap\"></a>\n\n| `simd-json` | json-small | 11 |\n";

    #[test]
    fn a_claim_the_row_holds_passes() {
        let readme = "sonic-rs takes [0.84×](report/README.md#kynos-decode \"json-small native (cycles)\") of the cycles.";
        assert!(check_claims(readme, REPORT).is_empty());
    }

    #[test]
    fn an_edited_figure_fails() {
        let readme = "sonic-rs takes [0.80×](report/README.md#kynos-decode \"json-small native (cycles)\").";
        assert_eq!(check_claims(readme, REPORT).len(), 1);
    }

    #[test]
    fn a_figure_from_another_section_fails() {
        let readme = "[11](report/README.md#kynos-decode \"simd-json json-small\")";
        assert_eq!(check_claims(readme, REPORT).len(), 1);
        let readme = "[11](report/README.md#heap \"simd-json json-small\")";
        assert!(check_claims(readme, REPORT).is_empty());
    }

    #[test]
    fn plain_links_are_not_claims() {
        assert!(check_claims("see [the heap table](report/README.md#heap)", REPORT).is_empty());
    }
}
