//! What this build and this CPU will actually run, and how quiet the host is.

use std::time::Duration;

use serde_json::{Value, json};

/// Target features this binary was compiled with that change a backend's
/// code path.
fn compiled_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    macro_rules! probe {
        ($($f:literal),*) => { $( if cfg!(target_feature = $f) { features.push($f); } )* };
    }
    probe!(
        "sse2",
        "ssse3",
        "sse4.1",
        "sse4.2",
        "popcnt",
        "avx",
        "avx2",
        "bmi1",
        "bmi2",
        "fma",
        "lzcnt",
        "pclmulqdq",
        "avx512f",
        "avx512bw"
    );
    features
}

/// Features the CPU reports at run time.
fn detected_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    macro_rules! detect {
        ($($f:tt),*) => { $( if std::is_x86_feature_detected!($f) { features.push($f); } )* };
    }
    detect!("sse4.2", "avx2", "bmi2", "pclmulqdq", "avx512f", "avx512bw");
    features
}

/// The code path each compiled backend takes on this build and CPU.
fn paths() -> Value {
    let mut paths = serde_json::Map::new();
    paths.insert("serde_json".into(), json!("scalar (SWAR + memchr)"));
    #[cfg(feature = "sonic-rs")]
    {
        let path = if cfg!(all(
            target_feature = "avx2",
            target_feature = "pclmulqdq",
            target_feature = "sse2"
        )) {
            "avx2+pclmulqdq"
        } else {
            "sse2 (fallback scanner)"
        };
        paths.insert("sonic-rs".into(), json!(path));
    }
    #[cfg(feature = "simd-json")]
    paths.insert(
        "simd-json".into(),
        json!(format!("{:?}", simd_json::Deserializer::algorithm())),
    );
    #[cfg(feature = "flexon-rt")]
    {
        let path = if std::is_x86_feature_detected!("avx2") {
            "rt: avx2 skip, sse2 scan"
        } else {
            "rt: sse2"
        };
        paths.insert("flexon".into(), json!(path));
    }
    #[cfg(feature = "flexon-ct")]
    {
        let path = if cfg!(all(target_feature = "avx2", target_feature = "pclmulqdq")) {
            "ct: avx2+pclmulqdq"
        } else if cfg!(target_feature = "sse4.2") {
            "ct: sse4.2"
        } else {
            "ct: sse2"
        };
        paths.insert("flexon".into(), json!(path));
    }
    #[cfg(feature = "jiter")]
    paths.insert("jiter".into(), json!("sse2"));
    Value::Object(paths)
}

/// The doctor report.
pub fn report() -> Value {
    json!({
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "compiled_features": compiled_features(),
        "detected_features": detected_features(),
        "backends": codecs::compiled(),
        "paths": paths(),
        "serde_json_features": {
            "float_roundtrip": cfg!(feature = "float-roundtrip"),
        },
    })
}

/// Pin this thread to `core`. Returns whether it worked.
pub fn pin(core: usize) -> bool {
    // SAFETY: `set` is a zeroed, then initialized, cpu_set_t owned by this
    // frame; the call reads it and nothing else.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(core, &mut set);
        libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set) == 0
    }
}

fn thread_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec owned by this frame.
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts);
    }
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Measures how much of a sample window something other than this thread
/// spent on the CPU the sample needed: the pinned core, and its SMT siblings
/// (`CELL_SIBLINGS`, comma-separated), which share its caches and pipeline.
/// Everything that ran on a sibling is interference.
#[derive(Debug)]
pub struct Interference {
    core: Option<usize>,
    busy: Option<u64>,
    siblings: Option<u64>,
    own: Duration,
}

/// The pinned core's SMT siblings, from `CELL_SIBLINGS`.
fn sibling_cores() -> &'static [usize] {
    static SIBLINGS: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    SIBLINGS.get_or_init(|| {
        std::env::var("CELL_SIBLINGS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|c| c.trim().parse().ok())
            .collect()
    })
}

impl Interference {
    /// How interference is detected.
    pub fn method() -> &'static str {
        "run time from /proc/schedstat on the pinned core (minus this thread's CPU time) and on its SMT siblings"
    }

    /// Start observing. `core` is the CPU the thread is pinned to, if any.
    pub fn start(core: Option<usize>) -> Self {
        Self {
            core,
            busy: busy_ns(core),
            siblings: siblings_ns(core),
            own: thread_cpu(),
        }
    }

    /// The fraction of `window` spent by other work.
    pub fn finish(self, window: Duration) -> f64 {
        let own = thread_cpu().saturating_sub(self.own);
        let core = match (self.busy, busy_ns(self.core)) {
            (Some(before), Some(after)) => after.saturating_sub(before) as f64 - own.as_nanos() as f64,
            _ => 0.0,
        };
        let siblings = match (self.siblings, siblings_ns(self.core)) {
            (Some(before), Some(after)) => after.saturating_sub(before) as f64,
            _ => 0.0,
        };
        ((core.max(0.0) + siblings) / window.as_nanos() as f64).max(0.0)
    }
}

/// Nanoseconds tasks have run on the siblings of `core`, if it is pinned.
fn siblings_ns(core: Option<usize>) -> Option<u64> {
    core?;
    sibling_cores().iter().map(|c| busy_ns(Some(*c))).sum()
}

/// Nanoseconds tasks have run on `core`.
fn busy_ns(core: Option<usize>) -> Option<u64> {
    let core = core?;
    let stat = std::fs::read_to_string("/proc/schedstat").ok()?;
    let line = stat.lines().find(|l| l.starts_with(&format!("cpu{core} ")))?;
    // cpuN <9 legacy fields> <run time ns> <wait time ns> <timeslices>
    line.split_whitespace().nth(7)?.parse().ok()
}
