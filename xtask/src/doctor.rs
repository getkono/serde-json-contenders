//! Proof that each build runs the code paths it claims, on this CPU.
//!
//! A `native` build is only a native build if the compiler saw the CPU's
//! features, and a SIMD backend is only measured as one if it took its SIMD
//! path. Both are checked, not assumed, and a run on a machine where either
//! fails is refused.

use std::process::Command;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::build::{Place, bin, build};
use crate::matrix::{SETS, variants};
use crate::util::{output, root, write_json};

/// `rustc --print cfg` target features for a `RUSTFLAGS` string.
fn rustc_features(rustflags: &str) -> Result<Vec<String>> {
    let mut cmd = Command::new("rustc");
    cmd.current_dir(root())
        .args(["--print", "cfg"])
        .args(rustflags.split_whitespace());
    Ok(output(&mut cmd)?
        .lines()
        .filter_map(|l| l.strip_prefix("target_feature=\""))
        .filter_map(|l| l.strip_suffix('"'))
        .map(str::to_owned)
        .collect())
}

/// What each backend's path must be, given the CPU and the build's features.
fn expected_path(set: &str, backend: &str, compiled: &[String], detected: &[String], arch: &str) -> Option<String> {
    let has = |list: &[String], f: &str| list.iter().any(|x| x == f);
    Some(match (backend, arch) {
        ("sonic-rs", "aarch64") => "neon".into(),
        ("sonic-rs", _) if has(compiled, "avx2") && has(compiled, "pclmulqdq") => "avx2+pclmulqdq".into(),
        ("sonic-rs", _) => "sse2 (fallback scanner)".into(),
        ("simd-json", "aarch64") => "NEON".into(),
        ("simd-json", _) if has(detected, "avx2") => "AVX2".into(),
        ("simd-json", _) if has(detected, "sse4.2") => "SSE42".into(),
        ("flexon", "aarch64") => "swar (no aarch64 SIMD)".into(),
        ("flexon", _) if set == "flexon-rt" && has(detected, "avx2") => "rt: avx2 skip, sse2 scan".into(),
        ("flexon", _) if set == "flexon-ct" && has(compiled, "avx2") && has(compiled, "pclmulqdq") => {
            "ct: avx2+pclmulqdq".into()
        }
        ("flexon", _) if set == "flexon-ct" && has(compiled, "sse4.2") => "ct: sse4.2".into(),
        _ => return None,
    })
}

/// `xtask doctor`: build every set at every variant on this host, ask each
/// `cell` what it compiled and what it will run, and check both.
pub fn doctor(args: &[String]) -> Result<()> {
    let only_native = args.iter().any(|a| a == "--native");
    let mut report = Vec::new();
    let mut failures = Vec::new();
    for variant in variants().into_iter().filter(|v| !only_native || v.name == "native") {
        let expected = rustc_features(variant.rustflags)?;
        for set in SETS {
            let release = build(Place::Host, "cell", set, &variant)?;
            let text = output(Command::new(bin(&release, "cell")).arg("doctor"))?;
            let doc: Value = serde_json::from_str(text.trim())?;
            let compiled: Vec<String> = doc["compiled_features"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            let detected: Vec<String> = doc["detected_features"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            // Every probed feature rustc enables for this variant must be
            // compiled in, and nothing it does not enable.
            let probed = [
                "sse2",
                "ssse3",
                "sse4.1",
                "sse4.2",
                "avx2",
                "bmi2",
                "pclmulqdq",
                "avx512f",
                "neon",
            ];
            for f in probed {
                let want = expected.iter().any(|e| e == f);
                let got = compiled.iter().any(|c| c == f);
                if want != got {
                    failures.push(format!(
                        "{} {}: target feature {f} expected {want}, compiled {got}",
                        variant.name, set.name
                    ));
                }
            }
            if let Value::Object(paths) = &doc["paths"] {
                for (backend, path) in paths {
                    if let Some(want) = expected_path(set.name, backend, &compiled, &detected, std::env::consts::ARCH)
                        && path.as_str() != Some(want.as_str())
                    {
                        failures.push(format!(
                            "{} {}: {backend} runs {path}, expected {want}",
                            variant.name, set.name
                        ));
                    }
                }
            }
            report.push(json!({ "variant": variant.name, "set": set.name, "rustc_features": expected, "cell": doc }));
        }
    }
    let path = root()
        .join("results")
        .join(crate::provenance::host_slug())
        .join("doctor.json");
    write_json(
        &path,
        &json!({ "schema": 1, "kind": "doctor", "provenance": crate::provenance::stamp(), "failures": failures, "builds": report }),
    )?;
    if failures.is_empty() {
        eprintln!(
            "doctor: every build compiled and runs the paths it claims ({})",
            path.display()
        );
        Ok(())
    } else {
        for f in &failures {
            eprintln!("doctor: {f}");
        }
        bail!("{} doctor failures", failures.len())
    }
}
