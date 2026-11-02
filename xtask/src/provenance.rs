//! What a result was measured on, stamped into every result file.

use std::fmt::Write as _;
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
    read("/proc/cpuinfo")
        .and_then(|info| {
            info.lines()
                .find(|l| l.starts_with("model name"))
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
/// The environment variable carrying the measured-code fingerprint into the
/// container.
pub const FINGERPRINT_VAR: &str = "SJC_FINGERPRINT";

/// Paths whose content cannot change a measurement: the generated report and
/// the code that writes it, the README, CI, and the results themselves. Every
/// other tracked file is measured code.
pub const UNMEASURED: &[&str] = &[
    ":!README.md",
    ":!results",
    ":!report",
    ":!xtask/src/report",
    ":!xtask/src/report.rs",
    ":!.github",
];

/// A hash of every measured file at HEAD (`git ls-tree`, so blob ids, not
/// working-tree content: a dirty checkout is flagged separately). Results
/// stamped with another fingerprint were measured on other code, and the
/// report refuses them; a commit that touches only [`UNMEASURED`] paths
/// keeps every result valid.
#[must_use]
pub fn fingerprint() -> Option<String> {
    use sha2::Digest as _;
    // `ls-tree` takes no exclude pathspecs, so the unmeasured paths are
    // filtered here.
    let (ok, tree, _) = capture(Command::new("git").current_dir(root()).args(["ls-tree", "-r", "HEAD"])).ok()?;
    let excluded: Vec<&str> = UNMEASURED.iter().map(|p| p.trim_start_matches(":!")).collect();
    let measured: String = tree
        .lines()
        .filter(|l| {
            let path = l.split_once('\t').map_or("", |(_, p)| p);
            !excluded
                .iter()
                .any(|x| path == *x || path.starts_with(&format!("{x}/")))
        })
        .flat_map(|l| [l, "\n"])
        .collect();
    ok.then(|| hex(&sha2::Sha256::digest(measured.as_bytes())[..8]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The commit the checkout is at, and whether the checkout differs from it,
/// read with git in the checkout. A commit git cannot read is `None`, and a
/// status it cannot read counts as dirty.
#[must_use]
pub fn git_state() -> (Option<String>, bool) {
    let commit = first_line(Command::new("git").current_dir(root()).args(["rev-parse", "HEAD"]));
    // Results, the report and the README are what a run writes or reads; any
    // other change means the measured code is not the committed code.
    let dirty = status_is_dirty(capture(
        Command::new("git")
            .current_dir(root())
            .args(["status", "--porcelain", "--", "."])
            .args(UNMEASURED),
    ));
    (commit, dirty)
}

/// Whether a `git status --porcelain` run says the checkout is dirty. A run
/// that could not start or exited non-zero counts as dirty, since its empty
/// stdout does not mean nothing changed.
fn status_is_dirty(status: anyhow::Result<(bool, String, String)>) -> bool {
    status
        .ok()
        .filter(|(ok, _, _)| *ok)
        .is_none_or(|(_, out, _)| !out.trim().is_empty())
}

/// The git state the host passed into the container through
/// [`GIT_COMMIT_VAR`] and [`GIT_DIRTY_VAR`]. Git cannot read the checkout from
/// inside it: a linked worktree's repository is not mounted, and a plain
/// checkout belongs to another user. Only an explicit `false` is clean.
fn passed_git_state(commit: Option<&str>, dirty: Option<&str>) -> (Option<String>, bool) {
    let commit = commit.map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned);
    (commit, dirty.map(str::trim) != Some("false"))
}

/// Every CPU's frequency governor, by CPU number, from sysfs. A CPU without
/// cpufreq (or a host without sysfs) has none, and is left out.
#[must_use]
pub fn governors() -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = std::fs::read_dir("/sys/devices/system/cpu")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let cpu: usize = e.file_name().to_str()?.strip_prefix("cpu")?.parse().ok()?;
            Some((
                cpu,
                read(&format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_governor"))?,
            ))
        })
        .collect();
    out.sort();
    out
}

/// The distinct governors among `governors`, joined: one word where every
/// CPU agrees. `None` where no CPU reports one.
fn governor_summary(governors: &[(usize, String)]) -> Option<String> {
    let mut distinct: Vec<&str> = governors.iter().map(|(_, g)| g.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    (!distinct.is_empty()).then(|| distinct.join(", "))
}

/// The 1-minute load as a timed stage started, once it had settled.
static STAGE_START_LOAD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Record the load as a timed stage starts; every stamp after it carries
/// that load as `loadavg_start` beside `loadavg`, the load as it was written.
pub fn stage_started() {
    if let Some(load) = read("/proc/loadavg") {
        let _ = STAGE_START_LOAD.set(load);
    }
}

/// The stamp taken when the task began.
static START: std::sync::OnceLock<Value> = std::sync::OnceLock::new();

/// Record the code a task is about to measure. Builds start from the
/// checkout as it is now, so a result is stamped with what this saw, not
/// with whatever the checkout holds when the task finishes hours later.
pub fn begin() {
    let _ = START.set(stamp());
}

/// [`stamp`], refused if the measured code changed since [`begin`]: a
/// commit or an edit in the middle of a run would otherwise label rows
/// measured on the old code with the new code's fingerprint.
pub fn stamp_checked() -> anyhow::Result<Value> {
    let now = stamp();
    if let Some(start) = START.get() {
        for k in ["git_commit", "git_dirty", "fingerprint"] {
            anyhow::ensure!(
                start[k] == now[k],
                "the measured code changed during the run ({k}: {} -> {}); rerun the task",
                start[k],
                now[k]
            );
        }
    }
    Ok(now)
}

/// Everything that could change a number.
#[must_use]
pub fn stamp() -> Value {
    let (commit, dirty, fingerprint) = if std::env::var_os("SJC_IN_CONTAINER").is_some() {
        let (commit, dirty) = passed_git_state(
            std::env::var(GIT_COMMIT_VAR).ok().as_deref(),
            std::env::var(GIT_DIRTY_VAR).ok().as_deref(),
        );
        (
            commit,
            dirty,
            std::env::var(FINGERPRINT_VAR).ok().filter(|f| !f.is_empty()),
        )
    } else {
        let (commit, dirty) = git_state();
        (commit, dirty, fingerprint())
    };
    let mut stamp = json!({
        "timestamp": now(),
        "git_commit": commit,
        "git_dirty": dirty,
        "fingerprint": fingerprint,
        "host": host_slug(),
        "cpu": cpu_model(),
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "kernel": first_line(Command::new("uname").arg("-r")),
        "rustc": capture(Command::new("rustc").arg("-vV")).map(|(_, o, _)| o).ok(),
        // Every CPU's, not only cpu0's: one in another mode moves the clock
        // of whatever it runs.
        "governor": governor_summary(&governors()),
        "boost": read("/sys/devices/system/cpu/cpufreq/boost"),
        "microcode": read("/proc/cpuinfo").and_then(|i| i.lines().find(|l| l.starts_with("microcode")).map(str::to_owned)),
        "container_image": std::env::var("SJC_IMAGE").ok(),
        "valgrind": first_line(Command::new("valgrind").arg("--version")),
        "loadavg": read("/proc/loadavg"),
    });
    if let Some(start) = STAGE_START_LOAD.get() {
        stamp["loadavg_start"] = json!(start);
    }
    stamp
}

#[cfg(test)]
mod tests {
    use super::{governor_summary, passed_git_state, status_is_dirty};

    const COMMIT: &str = "675987c9";

    #[test]
    fn summarises_every_cpus_governor() {
        let govs =
            |g: &[&str]| -> Vec<(usize, String)> { g.iter().enumerate().map(|(i, g)| (i, (*g).to_owned())).collect() };
        assert_eq!(
            governor_summary(&govs(&["performance", "performance"])).as_deref(),
            Some("performance")
        );
        assert_eq!(
            governor_summary(&govs(&["performance", "powersave", "performance"])).as_deref(),
            Some("performance, powersave")
        );
        assert_eq!(governor_summary(&[]), None);
    }

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

    fn status(ok: bool, out: &str) -> (bool, String, String) {
        (ok, out.to_owned(), String::new())
    }

    #[test]
    fn reads_a_successful_status_by_its_output() {
        assert!(!status_is_dirty(Ok(status(true, ""))));
        assert!(!status_is_dirty(Ok(status(true, "\n"))));
        assert!(status_is_dirty(Ok(status(true, " M xtask/src/provenance.rs\n"))));
    }

    /// A status that failed printed nothing, and that silence must not be
    /// read as a clean checkout.
    #[test]
    fn counts_a_failed_or_unspawned_status_as_dirty() {
        assert!(status_is_dirty(Ok(status(false, ""))));
        assert!(status_is_dirty(Err(anyhow::anyhow!("git not found"))));
    }
}
