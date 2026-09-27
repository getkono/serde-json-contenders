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
        "avx512bw",
        "neon",
        "aes",
        "sha2",
        "crc"
    );
    features
}

/// Features the CPU reports at run time.
fn detected_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    #[cfg(target_arch = "x86_64")]
    {
        macro_rules! detect {
            ($($f:tt),*) => { $( if std::is_x86_feature_detected!($f) { features.push($f); } )* };
        }
        detect!("sse4.2", "avx2", "bmi2", "pclmulqdq", "avx512f", "avx512bw");
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            features.push("neon");
        }
    }
    features
}

/// The code path each compiled backend takes on this build and CPU.
fn paths() -> Value {
    let mut paths = serde_json::Map::new();
    paths.insert("serde_json".into(), json!("scalar (SWAR + memchr)"));
    #[cfg(feature = "sonic-rs")]
    {
        let path = if cfg!(target_arch = "aarch64") {
            if cfg!(target_feature = "neon") {
                "neon"
            } else {
                "scalar"
            }
        } else if cfg!(all(
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
        #[cfg(target_arch = "x86_64")]
        let path = if std::is_x86_feature_detected!("avx2") {
            "rt: avx2 skip, sse2 scan"
        } else {
            "rt: sse2"
        };
        #[cfg(not(target_arch = "x86_64"))]
        let path = "swar (no aarch64 SIMD)";
        paths.insert("flexon".into(), json!(path));
    }
    #[cfg(feature = "flexon-ct")]
    {
        let path = if cfg!(target_arch = "x86_64") {
            if cfg!(all(target_feature = "avx2", target_feature = "pclmulqdq")) {
                "ct: avx2+pclmulqdq"
            } else if cfg!(target_feature = "sse4.2") {
                "ct: sse4.2"
            } else {
                "ct: sse2"
            }
        } else {
            "swar (no aarch64 SIMD)"
        };
        paths.insert("flexon".into(), json!(path));
    }
    #[cfg(feature = "jiter")]
    paths.insert(
        "jiter".into(),
        json!(if cfg!(target_arch = "aarch64") { "neon" } else { "sse2" }),
    );
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
#[cfg(target_os = "linux")]
pub fn pin(core: usize) -> bool {
    // SAFETY: `set` is a zeroed, then initialized, cpu_set_t owned by this
    // frame; the call reads it and nothing else.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(core, &mut set);
        libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set) == 0
    }
}

/// Apple Silicon ignores thread affinity; the scheduler places the thread.
#[cfg(not(target_os = "linux"))]
pub fn pin(_: usize) -> bool {
    false
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
/// spent on the CPU the sample needed.
#[derive(Debug)]
pub struct Interference {
    core: Option<usize>,
    busy: Option<u64>,
    own: Duration,
}

impl Interference {
    /// How interference is detected on this host.
    pub fn method() -> &'static str {
        if cfg!(target_os = "linux") {
            "per-core run time from /proc/schedstat minus this thread's CPU time"
        } else {
            "machine-wide busy ticks from host_processor_info minus this thread's CPU time"
        }
    }

    /// Start observing. `core` is the CPU the thread is pinned to, if any.
    pub fn start(core: Option<usize>) -> Self {
        Self {
            core,
            busy: busy_ns(core),
            own: thread_cpu(),
        }
    }

    /// The fraction of `window` spent by other work.
    pub fn finish(self, window: Duration) -> f64 {
        let own = thread_cpu().saturating_sub(self.own);
        match (self.busy, busy_ns(self.core)) {
            (Some(before), Some(after)) => {
                let other = after.saturating_sub(before) as f64 - own.as_nanos() as f64;
                (other / window.as_nanos() as f64 / scale()).max(0.0)
            }
            _ => 0.0,
        }
    }
}

/// On Linux the reading is one core's; elsewhere it is the whole machine's,
/// so it is normalized to one core's worth.
fn scale() -> f64 {
    if cfg!(target_os = "linux") {
        1.0
    } else {
        std::thread::available_parallelism().map_or(1.0, |n| n.get() as f64)
    }
}

/// Nanoseconds tasks have run on `core` (Linux) or on all cores (elsewhere).
#[cfg(target_os = "linux")]
fn busy_ns(core: Option<usize>) -> Option<u64> {
    let core = core?;
    let stat = std::fs::read_to_string("/proc/schedstat").ok()?;
    let line = stat.lines().find(|l| l.starts_with(&format!("cpu{core} ")))?;
    // cpuN <9 legacy fields> <run time ns> <wait time ns> <timeslices>
    line.split_whitespace().nth(7)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn busy_ns(_: Option<usize>) -> Option<u64> {
    let mut count: libc::natural_t = 0;
    let mut info: libc::processor_info_array_t = std::ptr::null_mut();
    let mut info_count: libc::mach_msg_type_number_t = 0;
    // SAFETY: the out-pointers are valid locals; the returned array is read
    // within its reported length and then deallocated exactly once.
    unsafe {
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        if libc::host_processor_info(
            host,
            libc::PROCESSOR_CPU_LOAD_INFO,
            &raw mut count,
            &raw mut info,
            &raw mut info_count,
        ) != 0
        {
            return None;
        }
        let ticks = std::slice::from_raw_parts(info, info_count as usize);
        let mut busy: u64 = 0;
        for cpu in ticks.chunks(libc::CPU_STATE_MAX as usize) {
            busy += (cpu[libc::CPU_STATE_USER as usize]
                + cpu[libc::CPU_STATE_SYSTEM as usize]
                + cpu[libc::CPU_STATE_NICE as usize]) as u64;
        }
        #[allow(deprecated)]
        libc::vm_deallocate(
            libc::mach_task_self(),
            info as libc::vm_address_t,
            (info_count as usize * size_of::<libc::integer_t>()) as libc::vm_size_t,
        );
        let hz = libc::sysconf(libc::_SC_CLK_TCK).max(1) as u64;
        Some(busy * 1_000_000_000 / hz)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn busy_ns(_: Option<usize>) -> Option<u64> {
    None
}
