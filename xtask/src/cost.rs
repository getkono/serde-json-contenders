//! What adopting a backend costs besides CPU: binary size, compile time, MSRV.

use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result};
use object::{Object, ObjectSection};
use serde_json::{Value, json};

use crate::build::{Place, bin, build};
use crate::counted::Filter;
use crate::util::{capture, root};

/// Size of the executable code section.
pub fn text_size(path: &std::path::Path) -> Result<u64> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let file = object::File::parse(&*data)?;
    let section = file
        .section_by_name(".text")
        .or_else(|| file.section_by_name("__text"))
        .context("no text section")?;
    Ok(section.size())
}

/// `xtask size`: `.text` of each fixture minus the floor, per set and variant.
pub fn size(args: &[String]) -> Result<()> {
    let filter = Filter::parse(args);
    let mut rows = Vec::new();
    for variant in filter.variants() {
        for set in filter.sets() {
            let release = build(Place::Host, "size-fixture", set, &variant)?;
            let floor = text_size(&bin(&release, "floor"))?;
            let full = text_size(&bin(&release, "fixture-full"))?;
            let decode = text_size(&bin(&release, "fixture-decode"))?;
            rows.push(json!({
                "variant": variant.name, "set": set.name, "crate": set.krate,
                "floor": floor, "full": full, "decode": decode,
                "full_delta": full.saturating_sub(floor), "decode_delta": decode.saturating_sub(floor),
            }));
        }
    }
    write("size", &rows)
}

fn write(kind: &str, rows: &[Value]) -> Result<()> {
    let path = root()
        .join("results")
        .join(crate::provenance::host_slug())
        .join(format!("{kind}.json"));
    crate::util::merge_write(&path, kind, rows, &["set", "variant"], json!({}))
}

fn timed_build(dir: &std::path::Path, set: &crate::matrix::Set, release: bool) -> Result<f64> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root())
        .args(["build", "--locked", "-p", "size-fixture", "--bin", "fixture-full"])
        .env("CARGO_TARGET_DIR", dir)
        // A compiler cache would measure the cache.
        .env("RUSTC_WRAPPER", "")
        .env_remove("RUSTFLAGS");
    if release {
        cmd.arg("--release");
    }
    if !set.features.is_empty() {
        cmd.args(["--features", set.features]);
    }
    let start = Instant::now();
    let (ok, _, stderr) = capture(&mut cmd)?;
    anyhow::ensure!(ok, "build failed: {stderr}");
    Ok(start.elapsed().as_secs_f64())
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Builds per measurement unless `--reps` says otherwise; the report states
/// any run that took fewer.
pub const COMPILE_REPS: usize = 5;

/// `xtask compile-time`: clean release, clean dev, and incremental dev
/// rebuild of the full fixture, `--reps` times each (default 5), median.
pub fn compile_time(args: &[String]) -> Result<()> {
    let filter = Filter::parse(args);
    let reps: usize = args
        .iter()
        .position(|a| a == "--reps")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(COMPILE_REPS);
    let touched = root().join("crates/size-fixture/src/lib.rs");
    let mut rows = Vec::new();
    for set in filter.sets() {
        let dir = root().join("target/compile-time").join(set.name);
        let (mut release, mut dev, mut incremental) = (Vec::new(), Vec::new(), Vec::new());
        for rep in 0..reps {
            eprintln!("compile-time: {} rep {}/{reps}", set.name, rep + 1);
            let _ = std::fs::remove_dir_all(&dir);
            release.push(timed_build(&dir, set, true)?);
            let _ = std::fs::remove_dir_all(&dir);
            dev.push(timed_build(&dir, set, false)?);
            // Touch: an mtime change rebuilds the fixture crate and nothing
            // beneath it, which is the edit-compile loop's shape.
            let text = std::fs::read(&touched)?;
            std::fs::write(&touched, &text)?;
            incremental.push(timed_build(&dir, set, false)?);
        }
        let _ = std::fs::remove_dir_all(&dir);
        rows.push(json!({
            "set": set.name, "crate": set.krate, "reps": reps,
            "release_s": median(release.clone()), "dev_s": median(dev.clone()), "incremental_s": median(incremental.clone()),
            "release_all": release, "dev_all": dev, "incremental_all": incremental,
            "jobs": std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        }));
    }
    write("compile-time", &rows)
}

/// The Rust version the MSRV probe compiles with (Kynos's MSRV).
pub const MSRV: &str = "1.85.0";

/// `xtask msrv`: does each backend (through the adapter, which uses nothing
/// newer than it) compile with Rust 1.85, and what does it declare?
pub fn msrv(args: &[String]) -> Result<()> {
    let filter = Filter::parse(args);
    let metadata: Value = serde_json::from_str(
        &crate::util::output(Command::new("cargo").current_dir(root()).args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
        ]))
        .or_else(|_| {
            // `--all-features` would unify flexon-rt and flexon-ct, which refuse
            // each other; the dependency list is the same either way.
            crate::util::output(Command::new("cargo").current_dir(root()).args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
            ]))
        })?,
    )?;
    let declared = |name: &str| {
        metadata["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["name"] == name)
            .map_or(Value::Null, |p| p["rust_version"].clone())
    };
    let mut rows = Vec::new();
    for set in filter.sets() {
        let dir = root().join("target/msrv").join(set.name);
        let mut cmd = Command::new("cargo");
        cmd.current_dir(root())
            .arg(format!("+{MSRV}"))
            .args(["check", "--locked", "-p", "codecs", "--ignore-rust-version"])
            .env("CARGO_TARGET_DIR", &dir)
            .env("RUSTC_WRAPPER", "")
            .env_remove("RUSTFLAGS");
        if !set.features.is_empty() {
            cmd.args(["--features", set.features]);
        }
        let (ok, _, stderr) = capture(&mut cmd)?;
        // The error and the file it points into, which names the crate that
        // needs the newer compiler.
        let mut lines = stderr.lines().skip_while(|l| !l.starts_with("error"));
        let first_error = lines.next().map(|e| {
            let at = lines
                .find(|l| l.trim_start().starts_with("-->"))
                .map(|l| l.trim().trim_start_matches("--> "));
            let at = at.map(|p| p.rsplit("/src/").nth(1).and_then(|c| c.rsplit('/').next()).unwrap_or(p));
            format!("{e} (in {})", at.unwrap_or("?"))
        });
        rows.push(json!({
            "set": set.name, "crate": set.krate, "declared": declared(set.krate),
            "builds_on_msrv": ok, "msrv": MSRV, "first_error": first_error,
        }));
    }
    write("msrv", &rows)
}
