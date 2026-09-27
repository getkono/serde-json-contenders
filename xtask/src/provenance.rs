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

/// Everything that could change a number.
#[must_use]
pub fn stamp() -> Value {
    let git = |args: &[&str]| first_line(Command::new("git").current_dir(root()).args(args));
    let dirty = capture(Command::new("git").current_dir(root()).args(["status", "--porcelain"]))
        .map_or(true, |(_, out, _)| !out.trim().is_empty());
    json!({
        "timestamp": now(),
        "git_commit": git(&["rev-parse", "HEAD"]),
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
