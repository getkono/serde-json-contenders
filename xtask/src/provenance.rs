//! What a result was measured on, stamped into every result file.

use std::process::Command;

use serde_json::{Value, json};

use crate::util::{capture, root};

fn first_line(cmd: &mut Command) -> Option<String> {
    capture(cmd)
        .ok()
        .filter(|(ok, _, _)| *ok)
        .map(|(_, out, _)| out.lines().next().unwrap_or("").trim().to_owned())
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

/// The CPU's marketing name.
#[must_use]
pub fn cpu_model() -> String {
    if cfg!(target_os = "macos") {
        return first_line(Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"])).unwrap_or_default();
    }
    read("/proc/cpuinfo")
        .and_then(|info| {
            info.lines()
                .find(|l| l.starts_with("model name") || l.starts_with("Model"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// A filesystem-safe identifier for this host's CPU and OS.
#[must_use]
pub fn host_slug() -> String {
    let model: String = cpu_model()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let model = model.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    format!("{}-{}-{model}", std::env::consts::ARCH, std::env::consts::OS)
}

/// UTC timestamp, RFC 3339, from the system clock.
#[must_use]
pub fn now() -> String {
    first_line(Command::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])).unwrap_or_default()
}

/// The environment variable carrying the host's commit into the container.
pub const GIT_COMMIT_VAR: &str = "SJC_GIT_COMMIT";
/// The environment variable carrying the host's dirty flag into the container.
pub const GIT_DIRTY_VAR: &str = "SJC_GIT_DIRTY";

/// The commit the checkout is at, and whether the checkout differs from it,
/// read with git in the checkout. A commit git cannot read is `None`, and a
/// status it cannot read counts as dirty.
#[must_use]
pub fn git_state() -> (Option<String>, bool) {
    let commit = first_line(Command::new("git").current_dir(root()).args(["rev-parse", "HEAD"]));
    // Results and the README they generate are what a run writes; any other
    // change means the measured code is not the committed code.
    let dirty = capture(Command::new("git").current_dir(root()).args([
        "status",
        "--porcelain",
        "--",
        ".",
        ":!results",
        ":!README.md",
    ]))
    .ok()
    .filter(|(ok, _, _)| *ok)
    .is_none_or(|(_, out, _)| !out.trim().is_empty());
    (commit, dirty)
}

/// The git state the host passed into the container through
/// [`GIT_COMMIT_VAR`] and [`GIT_DIRTY_VAR`]. Git cannot read the checkout from
/// inside it: a linked worktree's repository is not mounted, and a plain
/// checkout belongs to another user. Only an explicit `false` is clean.
fn passed_git_state(commit: Option<&str>, dirty: Option<&str>) -> (Option<String>, bool) {
    let commit = commit.map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned);
    (commit, dirty.map(str::trim) != Some("false"))
}

/// Everything that could change a number.
#[must_use]
pub fn stamp() -> Value {
    let (commit, dirty) = if std::env::var_os("SJC_IN_CONTAINER").is_some() {
        passed_git_state(
            std::env::var(GIT_COMMIT_VAR).ok().as_deref(),
            std::env::var(GIT_DIRTY_VAR).ok().as_deref(),
        )
    } else {
        git_state()
    };
    json!({
        "timestamp": now(),
        "git_commit": commit,
        "git_dirty": dirty,
        "host": host_slug(),
        "cpu": cpu_model(),
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "kernel": first_line(Command::new("uname").arg("-r")),
        "rustc": capture(Command::new("rustc").arg("-vV")).map(|(_, o, _)| o).ok(),
        "governor": read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        "boost": read("/sys/devices/system/cpu/cpufreq/boost"),
        "microcode": read("/proc/cpuinfo").and_then(|i| i.lines().find(|l| l.starts_with("microcode")).map(str::to_owned)),
        "container_image": std::env::var("SJC_IMAGE").ok(),
        "valgrind": first_line(Command::new("valgrind").arg("--version")),
        "loadavg": read("/proc/loadavg"),
    })
}

#[cfg(test)]
mod tests {
    use super::passed_git_state;

    const COMMIT: &str = "675987c9";

    #[test]
    fn records_the_commit_and_clean_flag_the_host_passed() {
        let commit = Some(COMMIT.to_owned());
        assert_eq!(
            passed_git_state(Some("675987c9\n"), Some("false")),
            (commit.clone(), false)
        );
        assert_eq!(passed_git_state(Some(COMMIT), Some("true")), (commit, true));
    }

    /// A container the host passed nothing to records no commit and does not
    /// claim the code it measured was committed.
    #[test]
    fn counts_a_missing_or_unreadable_state_as_dirty() {
        assert_eq!(passed_git_state(None, None), (None, true));
        assert_eq!(passed_git_state(Some(""), Some("")), (None, true));
        assert_eq!(
            passed_git_state(Some(COMMIT), Some("no")),
            (Some(COMMIT.to_owned()), true)
        );
    }
}
