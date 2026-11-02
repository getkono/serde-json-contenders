//! Isolated builds: one cargo invocation and one target directory per
//! (package, set, variant), with the resulting serde_json feature set checked.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::matrix::{Set, Variant};
use crate::util::{capture, root, run};

/// Whether an inherited variable would override what a build was told: cargo
/// prefers `CARGO_ENCODED_RUSTFLAGS` to `RUSTFLAGS`, and `CARGO_PROFILE_*` and
/// `CARGO_INCREMENTAL` to the profile.
fn overrides_build(name: &str) -> bool {
    name == "CARGO_ENCODED_RUSTFLAGS" || name == "CARGO_INCREMENTAL" || name.starts_with("CARGO_PROFILE_")
}

/// A `cargo` whose inherited environment cannot change what it builds.
/// `RUSTFLAGS` is set, empty until a caller names its flags, because a set one
/// also shadows `build.rustflags` and `target.*.rustflags` in a user's config,
/// and an unset one would not.
#[must_use]
pub fn cargo() -> Command {
    let mut cmd = Command::new("cargo");
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(overrides_build) {
            cmd.env_remove(name);
        }
    }
    cmd.env("RUSTFLAGS", "");
    cmd
}

/// `--config` arguments setting every key of `manifest`'s `[profile.release]`.
/// A config file can override the manifest's profile, and only the command
/// line outranks a config file.
fn pin_args(manifest: &str) -> Result<Vec<String>> {
    let manifest: toml::Table = manifest.parse().context("parsing the workspace manifest")?;
    let profile = manifest
        .get("profile")
        .and_then(|p| p.get("release"))
        .and_then(toml::Value::as_table)
        .context("the workspace manifest has no [profile.release]")?;
    Ok(profile
        .iter()
        .flat_map(|(key, value)| ["--config".to_owned(), format!("profile.release.{key}={value}")])
        .collect())
}

/// [`pin_args`] for this repository's manifest, as compiled in.
pub fn pinned_profile() -> Result<Vec<String>> {
    pin_args(include_str!("../../Cargo.toml"))
}

/// The profile settings cargo's config holds for a measured build: the pinned
/// release keys, plus whatever a user's config adds that a pin cannot reach,
/// such as `profile.release.package.*` or `profile.dev`. Stable cargo only
/// prints this behind `RUSTC_BOOTSTRAP`. `None` where cargo cannot say.
#[must_use]
pub fn effective_profile() -> Option<serde_json::Value> {
    let (ok, out, _) = capture(
        cargo()
            .current_dir(root())
            .env("RUSTC_BOOTSTRAP", "1")
            .args(["-Zunstable-options", "config", "get", "profile", "--format", "json"])
            .args(pinned_profile().ok()?),
    )
    .ok()?;
    if !ok {
        return None;
    }
    let config: serde_json::Value = serde_json::from_str(&out).ok()?;
    config.get("profile").cloned()
}

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

/// [`build`], with `feature` of `package` switched on as well, in its own
/// target directory (`<set>+<feature>`).
pub fn build_featured(place: Place, package: &str, set: &Set, variant: &Variant, feature: &str) -> Result<PathBuf> {
    let mut dir = target_dir(place, variant, set);
    dir.set_file_name(format!("{}+{feature}", set.name));
    let features = [set.features, feature]
        .iter()
        .filter(|f| !f.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(",");
    let mut cmd = cargo();
    cmd.current_dir(root())
        .args(["build", "--release", "--locked", "-p", package, "--features", &features])
        .args(pinned_profile()?)
        .env("RUSTFLAGS", variant.rustflags)
        .env("CARGO_TARGET_DIR", &dir);
    run(&mut cmd).with_context(|| format!("building {package} +{feature} for {} at {}", set.name, variant.name))?;
    check_serde_json_features(package, &features, set.name, set.serde_json_features)?;
    Ok(dir.join("release"))
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
    let mut cmd = cargo();
    cmd.current_dir(root())
        .args(["build", "--release", "--locked", "-p", package])
        .args(pinned_profile()?)
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
    let mut cmd = cargo();
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

#[cfg(test)]
mod tests {
    use super::{cargo, effective_profile, overrides_build, pin_args, pinned_profile};

    #[test]
    fn clears_only_what_overrides_a_build() {
        for name in [
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_INCREMENTAL",
            "CARGO_PROFILE_RELEASE_LTO",
            "CARGO_PROFILE_DEV_OPT_LEVEL",
        ] {
            assert!(overrides_build(name), "{name}");
        }
        for name in ["RUSTFLAGS", "CARGO_HOME", "CARGO_TARGET_DIR", "CARGO_PROFILE"] {
            assert!(!overrides_build(name), "{name}");
        }
    }

    /// Set, not merely unset: only a set `RUSTFLAGS` shadows a user's
    /// `build.rustflags`.
    #[test]
    fn cargo_sets_empty_rustflags() {
        let cmd = cargo();
        let flags = cmd.get_envs().find(|(k, _)| *k == "RUSTFLAGS").and_then(|(_, v)| v);
        assert_eq!(flags, Some(std::ffi::OsStr::new("")));
    }

    #[test]
    fn renders_each_release_key_as_a_config_argument() {
        let args = pin_args("[profile.release]\nlto = \"fat\"\nopt-level = 3\nrpath = false\n").unwrap();
        assert_eq!(
            args,
            [
                "--config",
                "profile.release.lto=\"fat\"",
                "--config",
                "profile.release.opt-level=3",
                "--config",
                "profile.release.rpath=false",
            ]
        );
        assert!(pin_args("[profile.test]\nopt-level = 2\n").is_err());
    }

    /// Every stable release setting, so that none is left for a user's config
    /// to supply.
    #[test]
    fn the_manifest_pins_every_stable_release_setting() {
        let args = pinned_profile().unwrap();
        for key in [
            "opt-level",
            "lto",
            "codegen-units",
            "panic",
            "debug",
            "strip",
            "split-debuginfo",
            "debug-assertions",
            "overflow-checks",
            "incremental",
            "rpath",
        ] {
            let prefix = format!("profile.release.{key}=");
            assert!(args.iter().any(|a| a.starts_with(&prefix)), "{key} is not pinned");
        }
    }

    #[test]
    fn reads_the_pinned_profile_back_from_cargo() {
        let profile = effective_profile().expect("cargo reports its profile config");
        assert_eq!(profile["release"]["lto"], "fat");
        assert_eq!(profile["release"]["opt-level"], 3);
    }
}
