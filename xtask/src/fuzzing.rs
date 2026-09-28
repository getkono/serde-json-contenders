//! Fuzzing and Miri: correctness runs that need a nightly toolchain.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::Result;
use serde_json::json;

use crate::counted::Filter;
use crate::util::{capture, root, run};

/// The nightly toolchain fuzzing and Miri need, pinned.
pub const NIGHTLY: &str = "nightly-2026-09-25";

/// Fuzz targets, one binary per backend feature.
pub const FUZZ_TARGETS: &[&str] = &["decode_value", "decode_struct", "roundtrip"];

/// `xtask fuzz [--seconds N] [--forks F] [--parallel P]`: every target
/// against every candidate and build configuration, each for N seconds of
/// wall time (default 1800) on F libFuzzer workers (default 2), P runs at
/// once (default 4). A run continues past every crash (`-ignore_crashes=1`),
/// so it always fuzzes its whole duration; its crashing inputs go to
/// `fuzz/artifacts/<set>/<config>/<target>/` and its corpus, minimized and
/// kept, to `fuzz/corpus/<set>/<config>/<target>`, so concurrent runs never
/// share either.
pub fn fuzz(args: &[String]) -> Result<()> {
    let flag = |name: &str, default: u64| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    // `-ignore_crashes` only takes effect in fork mode, so at least one worker.
    let (seconds, forks, parallel) = (
        flag("--seconds", 1800),
        flag("--forks", 2).max(1),
        flag("--parallel", 4),
    );
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
                    let corpus = fuzz.join("corpus").join(set.name).join(config).join(target);
                    let artifacts = fuzz.join("artifacts").join(set.name).join(config).join(target);
                    let _ = std::fs::create_dir_all(&corpus);
                    let _ = std::fs::create_dir_all(&artifacts);
                    let artifact_prefix = format!("-artifact_prefix={}/", artifacts.display());
                    let target_dir = root().join("target/fuzz").join(set.name).join(config);
                    let cargo_fuzz = |sub: &str| {
                        let mut cmd = Command::new("cargo");
                        cmd.current_dir(&fuzz)
                            .arg(format!("+{NIGHTLY}"))
                            .args(["fuzz", sub, "--target", &host, "--features", set.features, target])
                            .env("RUSTFLAGS", flags)
                            .env("CARGO_TARGET_DIR", &target_dir);
                        cmd
                    };
                    eprintln!("fuzz: {} {config} {target} for {seconds}s", set.name);
                    let started = std::time::SystemTime::now();
                    let (_, _, stderr) = capture(cargo_fuzz("run").arg(&corpus).args([
                        "--",
                        &format!("-max_total_time={seconds}"),
                        &format!("-fork={forks}"),
                        "-ignore_crashes=1",
                        "-seed=1",
                        &artifact_prefix,
                    ]))
                    .unwrap_or((false, String::new(), "spawn failed".into()));
                    // Every input this run found crashing, smallest first.
                    let crashes = crashes_since(&artifacts, started);
                    // Replay the smallest alone: forked workers' logs are not
                    // relayed past their `ERROR:` lines, so the assertion
                    // message and a full sanitizer report come from here.
                    let replay = crashes.first().map_or_else(String::new, |(path, _)| {
                        capture(cargo_fuzz("run").arg(path).args(["--", &artifact_prefix]))
                            .map(|(_, out, err)| format!("{out}\n{err}"))
                            .unwrap_or_default()
                    });
                    let found = Findings::read(&stderr, &replay, crashes.len());
                    let executed = stderr
                        .lines()
                        .rev()
                        .find_map(|l| l.split("exec/s").next().and_then(|h| h.split_whitespace().next()).filter(|t| t.starts_with('#')))
                        .map(str::to_owned);
                    // Minimize what was found into the corpus that is committed.
                    let _ = run(cargo_fuzz("cmin").arg(&corpus));
                    let (finished, tail) = finished_run(&stderr);
                    json!({
                        "set": set.name, "config": config, "target": target, "seconds": seconds, "forks": forks, "seed": 1,
                        "divergence_found": !finished || found.divergence, "executions": executed, "crash_count": found.crash_count,
                        "assertion": found.assertion,
                        "crash_input_hex": crashes.first().and_then(|(path, _)| std::fs::read(path).ok()).map(|b| hex(&b)),
                        "sanitizer_report": found.sanitizer_report, "crash_tail": tail,
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

/// The `crash-*` inputs in `dir` written at or after `since`, smallest first
/// (then by name, so the choice is stable).
fn crashes_since(dir: &Path, since: SystemTime) -> Vec<(PathBuf, u64)> {
    let mut crashes: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("crash-"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            (meta.modified().ok()? >= since).then(|| (e.path(), meta.len()))
        })
        .collect();
    crashes.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    crashes
}

/// Whether a run finished, and if it did not, the last 40 lines of its
/// stderr. The fork-mode parent exits with its last worker's code, so the
/// exit status says nothing; a run finished only if the parent reached its
/// exit line. One that did not (a build failure, a stopped parent, a spawn
/// failure) cannot pass rule 2, and its tail says why.
fn finished_run(stderr: &str) -> (bool, Option<String>) {
    let finished = stderr.lines().any(|l| l.starts_with("INFO: exiting:"));
    let tail = (!finished).then(|| {
        let lines: Vec<&str> = stderr.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    });
    (finished, tail)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// A sanitizer's memory-error line. A leak (`LeakSanitizer`) and a
/// `stack-overflow` are left out: safe Rust leaks, and safe recursion on
/// deeply nested input overflows its stack and aborts; neither is a
/// soundness defect. Both still crash, so both still count against rule 2.
fn is_sanitizer_error(line: &str) -> bool {
    line.contains("ERROR:")
        && line.contains("Sanitizer:")
        && !line.contains("LeakSanitizer")
        && !line.contains("Sanitizer: stack-overflow")
}

/// At most this many distinct relayed sanitizer lines are kept.
const RELAYED_LINES: usize = 8;

/// A relayed sanitizer line with what differs between workers removed: the
/// `==pid==` prefix and every hexadecimal address, so the same error from
/// many workers reads as one line.
fn normalise(line: &str) -> String {
    let line = line.find("ERROR:").map_or(line, |i| &line[i..]);
    line.split(' ')
        .map(|word| {
            let bare = word.trim_start_matches('(').trim_end_matches(')');
            if bare.starts_with("0x") && bare.len() > 2 && bare[2..].chars().all(|c| c.is_ascii_hexdigit()) {
                word.replace(bare, "0x?")
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What one fuzz run found, read from its fork-mode parent's stderr (which
/// relays only each crashing worker's `ERROR:` lines) and from replaying its
/// smallest crashing input alone.
#[derive(Debug, PartialEq, Eq)]
struct Findings {
    crash_count: usize,
    /// Some crash was not a sanitizer report: a panic, an assertion that
    /// fired on a disagreement with serde_json.
    divergence: bool,
    /// The first assertion message, from the replay.
    assertion: Option<String>,
    /// The replay's full sanitizer report, or failing that the distinct
    /// sanitizer lines the workers reported, pid and addresses removed, at
    /// most `RELAYED_LINES` of them.
    sanitizer_report: Option<String>,
}

impl Findings {
    fn read(stderr: &str, replay: &str, crash_count: usize) -> Self {
        let block = replay.lines().position(is_sanitizer_error).map(|start| {
            let lines: Vec<&str> = replay.lines().skip(start).collect();
            let end = lines
                .iter()
                .position(|l| l.starts_with("SUMMARY:"))
                .map_or(lines.len(), |i| i + 1);
            lines[..end.min(60)].join("\n")
        });
        let mut relayed: Vec<String> = Vec::new();
        let mut omitted = 0usize;
        for line in stderr.lines().filter(|l| is_sanitizer_error(l)).map(normalise) {
            if relayed.contains(&line) {
                continue;
            }
            if relayed.len() < RELAYED_LINES {
                relayed.push(line);
            } else {
                omitted += 1;
            }
        }
        if omitted > 0 {
            relayed.push(format!("... and {omitted} more distinct sanitizer lines"));
        }
        let sanitizer_report = block.or_else(|| (!relayed.is_empty()).then(|| relayed.join("\n")));
        let panicked = replay.contains("panicked at")
            || stderr
                .lines()
                .any(|l| l.contains("libFuzzer: deadly signal") || l.contains("libFuzzer: fuzz target exited"));
        let assertion = replay
            .lines()
            .skip_while(|l| !l.contains("panicked at"))
            .skip(1)
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            crash_count,
            divergence: panicked || (crash_count > 0 && sanitizer_report.is_none()),
            assertion: (!assertion.is_empty()).then_some(assertion),
            sanitizer_report,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{Findings, crashes_since, finished_run, hex};

    #[test]
    fn a_run_that_reached_its_exit_line_finished() {
        let stderr = "#99: cov: 1 exec/s: 9\n==1== ERROR: libFuzzer: deadly signal\nINFO: exiting: 77 time: 30s\n";
        assert_eq!(finished_run(stderr), (true, None));
    }

    #[test]
    fn a_run_without_an_exit_line_did_not_finish_and_keeps_its_tail() {
        let stderr = (0..50).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let (finished, tail) = finished_run(&stderr);
        assert!(!finished);
        let tail = tail.unwrap_or_default();
        assert_eq!(tail.lines().count(), 40);
        assert!(tail.starts_with("line 10\n"));
        assert!(tail.ends_with("line 49"));
    }

    #[test]
    fn a_spawn_failure_did_not_finish() {
        assert_eq!(finished_run("spawn failed"), (false, Some("spawn failed".into())));
    }

    const PANIC_RELAYED: &str = "#1024: cov: 10 ft: 12 corp: 3 exec/s: 512 oom/timeout/crash: 0/0/1 time: 3s\n\
        ==41== ERROR: libFuzzer: deadly signal\n";
    const PANIC_REPLAY: &str = "Running: crash-ab\n\
        thread '<unnamed>' panicked at fuzz_targets/decode_value.rs:12:5:\n\
        assertion `left == right` failed\n  left: Ok(1)\n right: Err\n\
        ==7== ERROR: libFuzzer: deadly signal\n";
    const ASAN_RELAYED: &str = "==42==ERROR: AddressSanitizer: heap-buffer-overflow on address 0x1\n";
    const ASAN_REPLAY: &str = "Running: crash-cd\n\
        ==9==ERROR: AddressSanitizer: heap-buffer-overflow on address 0x1\n\
        READ of size 1 at 0x1 thread T0\n    #0 0x2 in parse\n\
        SUMMARY: AddressSanitizer: heap-buffer-overflow src/lib.rs:3 in parse\n\
        ==9==ABORTING\n";

    #[test]
    fn a_clean_run_finds_nothing() {
        let found = Findings::read("#99: cov: 1 exec/s: 9\nINFO: exiting: 0 time: 30s\n", "", 0);
        assert_eq!(
            found,
            Findings {
                crash_count: 0,
                divergence: false,
                assertion: None,
                sanitizer_report: None
            }
        );
    }

    #[test]
    fn a_panic_is_a_divergence_with_its_assertion() {
        let found = Findings::read(PANIC_RELAYED, PANIC_REPLAY, 1);
        assert!(found.divergence);
        assert_eq!(found.crash_count, 1);
        assert_eq!(
            found.assertion.as_deref(),
            Some("assertion `left == right` failed   left: Ok(1)")
        );
        assert_eq!(found.sanitizer_report, None);
    }

    #[test]
    fn a_sanitizer_report_alone_is_not_a_divergence() {
        let found = Findings::read(ASAN_RELAYED, ASAN_REPLAY, 1);
        assert!(!found.divergence);
        let report = found.sanitizer_report.unwrap_or_default();
        assert!(report.starts_with("==9==ERROR: AddressSanitizer: heap-buffer-overflow"));
        assert!(report.ends_with("SUMMARY: AddressSanitizer: heap-buffer-overflow src/lib.rs:3 in parse"));
    }

    #[test]
    fn a_sanitizer_report_behind_a_smaller_panic_is_still_recorded() {
        let found = Findings::read(&format!("{PANIC_RELAYED}{ASAN_RELAYED}{ASAN_RELAYED}"), PANIC_REPLAY, 3);
        assert!(found.divergence);
        assert_eq!(
            found.sanitizer_report.as_deref(),
            Some("ERROR: AddressSanitizer: heap-buffer-overflow on address 0x?")
        );
    }

    #[test]
    fn relayed_lines_from_many_workers_collapse_to_one_per_error() {
        let stderr = "==11==ERROR: AddressSanitizer: heap-buffer-overflow on address 0x60200001 at pc 0x55aa bp 0x7ffc sp 0x7ffd\n\
            ==12==ERROR: AddressSanitizer: SEGV on unknown address 0x0000 (pc 0x55ab bp 0x7ffe sp 0x7fff T0)\n\
            ==13==ERROR: AddressSanitizer: heap-buffer-overflow on address 0x60200099 at pc 0x55bb bp 0x7ff0 sp 0x7ff1\n";
        let found = Findings::read(stderr, "", 3);
        assert_eq!(
            found.sanitizer_report.as_deref(),
            Some(
                "ERROR: AddressSanitizer: heap-buffer-overflow on address 0x? at pc 0x? bp 0x? sp 0x?\n\
                 ERROR: AddressSanitizer: SEGV on unknown address 0x? (pc 0x? bp 0x? sp 0x? T0)"
            )
        );
    }

    #[test]
    fn relayed_lines_are_capped() {
        let stderr = (0..20)
            .map(|i| format!("=={i}==ERROR: AddressSanitizer: kind-{i} on address 0x{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let report = Findings::read(&stderr, "", 20).sanitizer_report.unwrap_or_default();
        assert_eq!(report.lines().count(), super::RELAYED_LINES + 1);
        assert!(report.ends_with("... and 12 more distinct sanitizer lines"));
    }

    #[test]
    fn a_stack_overflow_is_not_a_sanitizer_report() {
        let found = Findings::read(
            "==6==ERROR: AddressSanitizer: stack-overflow on address 0x7ffe (pc 0x55 bp 0x7f sp 0x7e T0)\n",
            "",
            1,
        );
        assert_eq!(found.sanitizer_report, None);
        assert!(found.divergence);
    }

    #[test]
    fn a_leak_is_not_a_sanitizer_report() {
        let found = Findings::read("==5==ERROR: LeakSanitizer: detected memory leaks\n", "", 1);
        assert_eq!(found.sanitizer_report, None);
        assert!(found.divergence);
    }

    #[test]
    fn crashes_are_this_runs_only_smallest_first() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("xtask-fuzz-crashes-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("crash-old"), b"x")?;
        let old = std::fs::File::options().write(true).open(dir.join("crash-old"))?;
        old.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(1);
        std::fs::write(dir.join("crash-bb"), b"long")?;
        std::fs::write(dir.join("crash-aa"), b"ab")?;
        std::fs::write(dir.join("timeout-cc"), b"a")?;
        let names: Vec<String> = crashes_since(&dir, since)
            .into_iter()
            .map(|(p, _)| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(names, ["crash-aa", "crash-bb"]);
        Ok(())
    }

    #[test]
    fn hex_is_lowercase_pairs() {
        assert_eq!(hex(&[0x00, 0x7b, 0xff]), "007bff");
    }
}
