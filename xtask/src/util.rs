//! Process and file helpers.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The repository root.
#[must_use]
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Run a command, inheriting stdio; fail on a non-zero exit.
pub fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} exited with {status}");
    }
    Ok(())
}

/// Run a command and return its stdout; fail on a non-zero exit.
pub fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("spawning {cmd:?}"))?;
    if !out.status.success() {
        bail!("{cmd:?} exited with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a command and return (success, stdout, stderr) without failing.
pub fn capture(cmd: &mut Command) -> Result<(bool, String, String)> {
    let out = cmd.output().with_context(|| format!("spawning {cmd:?}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Write pretty JSON, creating parent directories.
pub fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Read JSON.
pub fn read_json(path: &Path) -> Result<serde_json::Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

/// A unit of work for [`parallel`].
pub type Job<'a, T> = Box<dyn FnOnce() -> T + Send + 'a>;

/// Run `jobs` closures on `threads` threads, preserving order of results.
pub fn parallel<T: Send>(threads: usize, jobs: Vec<Job<'_, T>>) -> Vec<T> {
    let jobs: std::sync::Mutex<Vec<(usize, Job<'_, T>)>> =
        std::sync::Mutex::new(jobs.into_iter().enumerate().rev().collect());
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let next = jobs.lock().map(|mut j| j.pop()).ok().flatten();
                    let Some((i, job)) = next else { break };
                    let result = job();
                    if let Ok(mut r) = results.lock() {
                        r.push((i, result));
                    }
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_default();
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

/// A deterministic shuffle (Fisher-Yates over splitmix64).
pub fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = payloads::Rng::new(seed);
    for i in (1..items.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        items.swap(i, j);
    }
}

/// Write `rows` into the result file at `path`, replacing earlier rows whose
/// `keys` fields match a new row's, and stamp provenance. Partial reruns
/// therefore update in place instead of dropping what they did not rerun.
pub fn merge_write(
    path: &Path,
    kind: &str,
    rows: &[serde_json::Value],
    keys: &[&str],
    extra: serde_json::Value,
) -> Result<()> {
    let key = |r: &serde_json::Value| keys.iter().map(|k| r[*k].to_string()).collect::<Vec<_>>().join("|");
    let fresh: std::collections::HashSet<String> = rows.iter().map(key).collect();
    let mut all: Vec<serde_json::Value> = read_json(path)
        .ok()
        .and_then(|v| v["rows"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| !fresh.contains(&key(r)))
        .collect();
    all.extend(rows.iter().cloned());
    all.sort_by_key(key);
    let mut doc =
        serde_json::json!({ "schema": 1, "kind": kind, "provenance": crate::provenance::stamp(), "rows": all });
    if let (serde_json::Value::Object(doc), serde_json::Value::Object(extra)) = (&mut doc, extra) {
        doc.extend(extra);
    }
    write_json(path, &doc)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}
