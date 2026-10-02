//! Running a task inside the pinned valgrind image.

use std::process::Command;

use anyhow::Result;

use crate::provenance;
use crate::util::{output, root, run};

/// The image tag `xtask image` builds from `Containerfile`.
pub const IMAGE: &str = "serde-json-contenders:valgrind";

/// The container engine: podman, else docker.
fn engine() -> &'static str {
    if Command::new("podman").arg("--version").output().is_ok() {
        "podman"
    } else {
        "docker"
    }
}

/// `xtask image`: build the image.
pub fn image() -> Result<()> {
    run(Command::new(engine())
        .current_dir(root())
        .args(["build", "-t", IMAGE, "-f", "Containerfile", "."]))
}

/// Re-run `xtask <task> <args>` inside the image, with the repository and the
/// cargo registry mounted.
///
/// Core dumps are off: valgrind writes a `vgcore.<pid>` into the working
/// directory, the mounted repository, for every client a signal kills, so a
/// variant it cannot decode (SVE and `ldapr` on aarch64, AVX-512 on x86-64) would
/// otherwise leave one core per cell until the disk fills. The cell's row
/// records the failure; a core file adds nothing to it.
pub fn reexec(task: &str, args: &[String]) -> Result<()> {
    if !cfg!(target_os = "linux") {
        anyhow::bail!("{task} runs valgrind, which does not run on this OS; run it on Linux (or let CI run it)");
    }
    image()?;
    let id = output(Command::new(engine()).args(["image", "inspect", "--format", "{{.Id}}", IMAGE]))?;
    let home = std::env::var("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::Path::new(&h).join(".cargo")))?;
    // Git cannot read the mounted checkout from inside the container, so the
    // host reads it here and passes what the stamp records.
    let (commit, dirty) = provenance::git_state();
    let mut git = vec!["-e".to_owned(), format!("{}={dirty}", provenance::GIT_DIRTY_VAR)];
    if let Some(commit) = commit {
        git.extend(["-e".to_owned(), format!("{}={commit}", provenance::GIT_COMMIT_VAR)]);
    }
    run(Command::new(engine())
        .args(["run", "--rm", "--security-opt", "label=disable", "--ulimit", "core=0"])
        .args(&git)
        .arg("-v")
        .arg(format!("{}:/work", root().display()))
        .arg("-v")
        .arg(format!("{}:/usr/local/cargo/registry", home.join("registry").display()))
        .args(["-e", "SJC_IN_CONTAINER=1", "-e"])
        .arg(format!("SJC_IMAGE={}", id.trim()))
        .arg(IMAGE)
        .args([
            "cargo",
            "run",
            "--quiet",
            "--release",
            "--locked",
            "-p",
            "xtask",
            "--",
            task,
        ])
        .args(args))
}
