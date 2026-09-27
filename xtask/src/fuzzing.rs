//! Fuzzing and Miri: correctness runs that need a nightly toolchain.

use std::process::Command;

use anyhow::Result;
use serde_json::json;

use crate::counted::Filter;
use crate::util::{capture, root, run};

/// The nightly toolchain fuzzing and Miri need, pinned.
pub const NIGHTLY: &str = "nightly-2026-09-25";

/// Fuzz targets, one binary per backend feature.
pub const FUZZ_TARGETS: &[&str] = &["decode_value", "decode_struct", "roundtrip"];

/// `xtask fuzz [--seconds N] [--forks F] [--parallel P]`: every target
/// against every candidate, each for N seconds of wall time (default 1800)
/// on F libFuzzer workers (default 2), P runs at once (default 4); corpora
/// minimized and kept.
pub fn fuzz(args: &[String]) -> Result<()> {
    let flag = |name: &str, default: u64| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let (seconds, forks, parallel) = (flag("--seconds", 1800), flag("--forks", 2), flag("--parallel", 4));
    let filter = Filter::parse(args);
    let fuzz = root().join("fuzz");
    // cargo-fuzz's release binary defaults to its own (musl) triple, where
    // the sanitizer cannot link; name the host's.
    let host = crate::util::output(Command::new("rustc").arg("-vV"))?
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_owned)
        .unwrap_or_default();
    let mut jobs: Vec<crate::util::Job<'_, serde_json::Value>> = Vec::new();
    for set in filter.sets().into_iter().filter(|s| !s.name.starts_with("baseline")) {
        let configs: &[(&str, &str)] = if set.name == "sonic-rs" {
            &[
                ("v3", "-C target-cpu=x86-64-v3 -C target-feature=+pclmulqdq"),
                ("portable", "-C target-cpu=x86-64"),
            ]
        } else {
            &[("v3", "-C target-cpu=x86-64-v3 -C target-feature=+pclmulqdq")]
        };
        for &(config, flags) in configs {
            let flags = if cfg!(target_arch = "x86_64") { flags } else { "" };
            for &target in FUZZ_TARGETS {
                let fuzz = fuzz.clone();
                let host = host.clone();
                jobs.push(Box::new(move || {
                    let corpus = fuzz.join("corpus").join(set.name).join(target);
                    let _ = std::fs::create_dir_all(&corpus);
                    let target_dir = root().join("target/fuzz").join(set.name).join(config);
                    eprintln!("fuzz: {} {config} {target} for {seconds}s", set.name);
                    let mut cmd = Command::new("cargo");
                    cmd.current_dir(&fuzz)
                        .arg(format!("+{NIGHTLY}"))
                        .args(["fuzz", "run", "--target", &host, "--features", set.features, target])
                        .arg(&corpus)
                        .args(["--", &format!("-max_total_time={seconds}"), &format!("-fork={forks}"), "-seed=1"])
                        .env("RUSTFLAGS", flags)
                        .env("CARGO_TARGET_DIR", &target_dir);
                    let (ok, _, stderr) = capture(&mut cmd).unwrap_or((false, String::new(), "spawn failed".into()));
                    // The divergence: the assertion that fired, and the input.
                    let assertion = stderr
                        .lines()
                        .skip_while(|l| !l.contains("panicked at"))
                        .skip(1)
                        .take(2)
                        .collect::<Vec<_>>()
                        .join(" ");
                    let crash = stderr
                        .lines()
                        .find_map(|l| l.split("Test unit written to ").nth(1))
                        .map(|p| fuzz.join(p.trim()))
                        .and_then(|p| std::fs::read(p).ok())
                        .map(|bytes| {
                            use std::fmt::Write as _;
                            bytes.iter().fold(String::new(), |mut hex, b| {
                                let _ = write!(hex, "{b:02x}");
                                hex
                            })
                        });
                    let executed = stderr
                        .lines()
                        .rev()
                        .find_map(|l| l.split("exec/s").next().and_then(|h| h.split_whitespace().next()).filter(|t| t.starts_with('#')))
                        .map(str::to_owned);
                    // Minimize what was found into the corpus that is committed.
                    let _ = run(Command::new("cargo")
                        .current_dir(&fuzz)
                        .arg(format!("+{NIGHTLY}"))
                        .args(["fuzz", "cmin", "--target", &host, "--features", set.features, target])
                        .arg(&corpus)
                        .env("RUSTFLAGS", flags)
                        .env("CARGO_TARGET_DIR", &target_dir));
                    json!({
                        "set": set.name, "config": config, "target": target, "seconds": seconds, "forks": forks, "seed": 1,
                        "divergence_found": !ok, "executions": executed, "assertion": (!ok).then_some(assertion), "crash_input_hex": crash,
                        "crash_tail": if ok { None } else { Some(stderr.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")) },
                    })
                }));
            }
        }
    }
    let rows = crate::util::parallel(usize::try_from(parallel).unwrap_or(1), jobs);
    crate::util::merge_write(
        &root().join("results/fuzz.json"),
        "fuzz",
        &rows,
        &["set", "config", "target"],
        json!({ "toolchain": NIGHTLY }),
    )
}

/// `xtask miri`: the conformance crate's Miri subset for every candidate.
pub fn miri(args: &[String]) -> Result<()> {
    let filter = Filter::parse(args);
    let mut rows = Vec::new();
    for set in filter.sets() {
        eprintln!("miri: {}", set.name);
        let mut cmd = Command::new("cargo");
        cmd.current_dir(root())
            .arg(format!("+{NIGHTLY}"))
            .args(["miri", "test", "--locked", "-p", "conformance", "--test", "miri"])
            .env("CARGO_TARGET_DIR", root().join("target/miri").join(set.name))
            .env("MIRIFLAGS", "-Zmiri-disable-isolation")
            .env_remove("RUSTFLAGS");
        if !set.features.is_empty() {
            cmd.args(["--features", set.features]);
        }
        let (ok, stdout, stderr) = capture(&mut cmd)?;
        let text = format!("{stdout}\n{stderr}");
        let outcome = if ok {
            "pass"
        } else if text.contains("Undefined Behavior") {
            "undefined-behavior"
        } else if text.contains("unsupported operation") || text.contains("can't call foreign function") {
            "unsupported"
        } else {
            "error"
        };
        let detail = text
            .lines()
            .find(|l| l.contains("Undefined Behavior") || l.contains("unsupported operation") || l.starts_with("error"))
            .map(str::to_owned);
        rows.push(json!({ "set": set.name, "outcome": outcome, "detail": detail }));
    }
    crate::util::merge_write(
        &root().join("results/miri.json"),
        "miri",
        &rows,
        &["set"],
        json!({ "toolchain": NIGHTLY }),
    )
}
