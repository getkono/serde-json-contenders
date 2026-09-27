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
    /// Whether valgrind can run what this variant emits on the given host.
    /// `native` on an AVX-512 host cannot be.
    pub valgrind: bool,
}

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
                valgrind: true,
            },
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
