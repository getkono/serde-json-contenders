//! The build matrix: which backend crates are built how.
//!
//! A *set* is one cargo feature set: at most one candidate crate beside
//! serde_json, so no candidate can change another's features through
//! unification. A *variant* is one `-C target-cpu`. Every set is built at
//! every variant, the baseline included: a comparison at unequal
//! `target-cpu` is not a comparison.

/// One cargo feature set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Set {
    /// Stable identifier, also the target directory name.
    pub name: &'static str,
    /// Features passed to `cell`, `conformance`, `e2e`, `size-fixture`.
    pub features: &'static str,
    /// Backends this set measures (the reference is measured only in the
    /// baseline sets).
    pub backends: &'static [&'static str],
    /// The crate whose cost this set weighs (`size`, `compile-time`, `msrv`).
    pub krate: &'static str,
    /// The serde_json features a build of this set must end up with.
    pub serde_json_features: &'static [&'static str],
}

const DEFAULT_SJ: &[&str] = &["default", "std"];

/// Every set, baseline first.
pub const SETS: &[Set] = &[
    Set {
        name: "baseline",
        features: "",
        backends: &["serde_json"],
        krate: "serde_json",
        serde_json_features: DEFAULT_SJ,
    },
    Set {
        name: "baseline-float-roundtrip",
        features: "float-roundtrip",
        backends: &["serde_json"],
        krate: "serde_json",
        serde_json_features: &["default", "float_roundtrip", "std"],
    },
    Set {
        name: "sonic-rs",
        features: "sonic-rs",
        backends: &["sonic-rs"],
        krate: "sonic-rs",
        serde_json_features: DEFAULT_SJ,
    },
    Set {
        name: "simd-json",
        features: "simd-json",
        backends: &["simd-json", "simd-json-buffers"],
        krate: "simd-json",
        serde_json_features: DEFAULT_SJ,
    },
    Set {
        name: "flexon-rt",
        features: "flexon-rt",
        backends: &["flexon-rt", "flexon-rt-mut"],
        krate: "flexon",
        serde_json_features: DEFAULT_SJ,
    },
    Set {
        name: "flexon-ct",
        features: "flexon-ct",
        backends: &["flexon-ct", "flexon-ct-mut"],
        krate: "flexon",
        serde_json_features: DEFAULT_SJ,
    },
    Set {
        name: "jiter",
        features: "jiter",
        backends: &["jiter"],
        krate: "jiter",
        serde_json_features: DEFAULT_SJ,
    },
    Set {
        name: "hifijson",
        features: "hifijson",
        backends: &["hifijson"],
        krate: "hifijson",
        serde_json_features: DEFAULT_SJ,
    },
    Set {
        name: "struson",
        features: "struson",
        backends: &["struson"],
        krate: "struson",
        serde_json_features: DEFAULT_SJ,
    },
];

/// Look a set up by name.
pub fn set(name: &str) -> anyhow::Result<&'static Set> {
    SETS.iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("no set named {name}"))
}

/// Label a backend as reported: the float-roundtrip baseline is its own row.
#[must_use]
pub fn label(set: &Set, backend: &str) -> String {
    if set.name == "baseline-float-roundtrip" {
        "serde_json+float_roundtrip".into()
    } else {
        backend.into()
    }
}

/// One `-C target-cpu` choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    /// Stable identifier.
    pub name: &'static str,
    /// `RUSTFLAGS`.
    pub rustflags: &'static str,
    /// Whether the counted (callgrind) tasks run this variant on the given
    /// host. `native` on an AVX-512 host cannot be counted, and aarch64
    /// `native` is counted as [`ARM_NATIVE_COUNTED`] instead.
    pub valgrind: bool,
}

/// aarch64 `native` with the target features valgrind's arm64 decoder cannot
/// run (up to at least 3.25.1) switched off: `sve`, whose removal takes
/// `sve2` with it, and `rcpc` (the `ldapr` loads), whose removal takes
/// `rcpc2` with it. Neoverse N2, GitHub's `ubuntu-24.04-arm`, has both, and
/// fat LTO recompiles std with them, so a `native` build dies in valgrind
/// before any measured work. This is the build rule 1's aarch64 counts are
/// taken from; every other feature of the CPU, and its tuning, are the
/// `native` build's.
pub const ARM_NATIVE_COUNTED: Variant = Variant {
    name: "native-counted",
    rustflags: "-C target-cpu=native -C target-feature=-sve,-rcpc",
    valgrind: true,
};

/// The variants for the architecture this binary runs on.
#[must_use]
pub fn variants() -> Vec<Variant> {
    if cfg!(target_arch = "aarch64") {
        vec![
            Variant {
                name: "portable",
                rustflags: "-C target-cpu=generic",
                valgrind: true,
            },
            Variant {
                name: "native",
                rustflags: "-C target-cpu=native",
                valgrind: false,
            },
            ARM_NATIVE_COUNTED,
        ]
    } else {
        vec![
            Variant {
                name: "portable",
                rustflags: "-C target-cpu=x86-64",
                valgrind: true,
            },
            Variant {
                name: "v3",
                rustflags: "-C target-cpu=x86-64-v3 -C target-feature=+pclmulqdq",
                valgrind: true,
            },
            Variant {
                name: "native",
                rustflags: "-C target-cpu=native",
                valgrind: !host_has_avx512(),
            },
        ]
    }
}

/// Look a variant up by name.
pub fn variant(name: &str) -> anyhow::Result<Variant> {
    variants()
        .into_iter()
        .find(|v| v.name == name)
        .ok_or_else(|| anyhow::anyhow!("no variant {name} on this arch"))
}

fn host_has_avx512() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("avx512f")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// An operation cell's shape, as `cell` takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// `decode`, `encode`, `encode-into`.
    pub op: &'static str,
    /// `owned`, `borrowed`.
    pub form: &'static str,
    /// `shared`, `unique`.
    pub arrival: &'static str,
}

/// Every operation shape. Encodes have no form or arrival of their own.
pub const SHAPES: &[Shape] = &[
    Shape {
        op: "decode",
        form: "owned",
        arrival: "shared",
    },
    Shape {
        op: "decode",
        form: "owned",
        arrival: "unique",
    },
    Shape {
        op: "decode",
        form: "borrowed",
        arrival: "shared",
    },
    Shape {
        op: "decode",
        form: "borrowed",
        arrival: "unique",
    },
    Shape {
        op: "encode",
        form: "owned",
        arrival: "shared",
    },
    Shape {
        op: "encode-into",
        form: "owned",
        arrival: "shared",
    },
];

/// The shapes the timed and end-to-end protocols run: the decode a server
/// does (owned, one shared frame) and the encode it does (`to_vec`).
pub const TIMED_SHAPES: &[Shape] = &[SHAPES[0], SHAPES[4]];

/// The workloads the decision rule is evaluated on.
pub const KYNOS: &[&str] = &["json-small", "echo-post", "json-large"];

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::ARM_NATIVE_COUNTED;

    /// rustc's target features for aarch64 Linux at `rustflags`, with
    /// `native` read as Neoverse N2 (GitHub's `ubuntu-24.04-arm`), so the
    /// check runs on any host.
    fn n2_features(rustflags: &str) -> Vec<String> {
        let flags = rustflags.replace("target-cpu=native", "target-cpu=neoverse-n2");
        let out = Command::new("rustc")
            .args(["--print", "cfg", "--target", "aarch64-unknown-linux-gnu"])
            .args(flags.split_whitespace())
            .output()
            .expect("rustc runs");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.strip_prefix("target_feature=\"")?.strip_suffix('"'))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn the_counted_arm_build_switches_off_exactly_what_valgrind_cannot_decode() {
        let native = n2_features("-C target-cpu=native");
        let counted = n2_features(ARM_NATIVE_COUNTED.rustflags);
        assert!(counted.iter().all(|c| native.contains(c)), "{counted:?}");
        let dropped: Vec<&str> = native
            .iter()
            .filter(|n| !counted.contains(n))
            .map(String::as_str)
            .collect();
        assert_eq!(dropped, ["rcpc", "rcpc2", "sve", "sve2"]);
        assert!(counted.iter().any(|c| c == "neon"));
    }
}
