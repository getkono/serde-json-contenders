//! Running a task inside the pinned valgrind image.

use std::process::Command;

use anyhow::Result;

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
pub fn reexec(task: &str, args: &[String]) -> Result<()> {
    if !cfg!(target_os = "linux") {
        anyhow::bail!("{task} runs valgrind, which does not run on this OS; run it on Linux (or let CI run it)");
    }
    image()?;
    let id = output(Command::new(engine()).args(["image", "inspect", "--format", "{{.Id}}", IMAGE]))?;
    let home = std::env::var("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::Path::new(&h).join(".cargo")))?;
    run(Command::new(engine())
        .args(["run", "--rm", "--security-opt", "label=disable"])
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
