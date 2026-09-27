//! Isolated builds: one cargo invocation and one target directory per
//! (package, set, variant), with the resulting serde_json feature set checked.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::matrix::{Set, Variant};
use crate::util::{root, run};

/// Where builds happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// On this machine.
    Host,
    /// Inside the valgrind container (its glibc).
    Container,
}

/// The target directory for one build.
#[must_use]
pub fn target_dir(place: Place, variant: &Variant, set: &Set) -> PathBuf {
    let place = match place {
        Place::Host => "host",
        Place::Container => "container",
    };
    root()
        .join("target/matrix")
        .join(place)
        .join(variant.name)
        .join(set.name)
}

/// Build `package` for `set` at `variant`; returns the release directory.
pub fn build(place: Place, package: &str, set: &Set, variant: &Variant) -> Result<PathBuf> {
    build_with(place, package, set, variant, None)
}

/// [`build`], with serde_json's `arbitrary_precision` unified in when
/// `arbitrary_precision` is set — the build a program gets when any crate in
/// it asks for that feature.
pub fn build_with(
    place: Place,
    package: &str,
    set: &Set,
    variant: &Variant,
    arbitrary_precision: Option<()>,
) -> Result<PathBuf> {
    let mut dir = target_dir(place, variant, set);
    let mut features = set.features.to_owned();
    if arbitrary_precision.is_some() {
        dir.set_file_name(format!("{}+arbitrary-precision", set.name));
        features = [set.features, "arbitrary-precision"]
            .iter()
            .filter(|f| !f.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(",");
    }
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root())
        .args(["build", "--release", "--locked", "-p", package])
        .env("RUSTFLAGS", variant.rustflags)
        .env("CARGO_TARGET_DIR", &dir);
    if !features.is_empty() {
        cmd.args(["--features", &features]);
    }
    run(&mut cmd).with_context(|| format!("building {package} for {} at {}", set.name, variant.name))?;
    let mut want: Vec<&str> = set.serde_json_features.to_vec();
    if arbitrary_precision.is_some() {
        want.push("arbitrary_precision");
    }
    check_serde_json_features(package, &features, set.name, &want)?;
    Ok(dir.join("release"))
}

/// The serde_json features cargo resolves for `package` with `features`.
pub fn serde_json_features(package: &str, features: &str) -> Result<Vec<String>> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root()).args([
        "tree",
        "--locked",
        "-p",
        package,
        "-e",
        "features,normal",
        "-i",
        "serde_json",
        "--prefix",
        "none",
    ]);
    if !features.is_empty() {
        cmd.args(["--features", features]);
    }
    let out = crate::util::output(&mut cmd)?;
    let mut features: Vec<String> = out
        .lines()
        .filter_map(|l| l.strip_prefix("serde_json feature \""))
        .filter_map(|l| l.split('"').next())
        .map(str::to_owned)
        .collect();
    features.sort();
    features.dedup();
    Ok(features)
}

/// Refuse a build whose serde_json features differ from what its set names:
/// a candidate that silently switched on `float_roundtrip` would slow the
/// baseline it is compared against.
pub fn check_serde_json_features(package: &str, features: &str, set: &str, want: &[&str]) -> Result<()> {
    let got = serde_json_features(package, features)?;
    let mut want: Vec<String> = want.iter().map(|s| (*s).to_owned()).collect();
    want.sort();
    if got != want {
        bail!("{package} in set {set}: serde_json features {got:?}, expected {want:?}");
    }
    Ok(())
}

/// Path to a built binary.
#[must_use]
pub fn bin(release: &Path, name: &str) -> PathBuf {
    release.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}
