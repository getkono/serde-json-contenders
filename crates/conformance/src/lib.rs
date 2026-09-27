//! Differential conformance: every compiled backend against serde_json, in
//! the same build.
//!
//! Each section module proves one property a server relies on when it swaps
//! serde_json for a backend: that it accepts and rejects the same bodies,
//! decodes them to the same values, answers the same status, survives the
//! same nesting, rounds numbers correctly, and encodes the same JSON. A
//! backend misbehaving is a finding, never a crash of the suite: every decode
//! and encode runs under `catch_unwind`, depth probes run in a subprocess,
//! and the `conform` binary runs each backend's suite in its own process.
//!
//! "Agree" means the same accept/reject, the same decoded value exactly
//! (floats by bit pattern), and the same status (400 or 422).

pub mod attributes;
pub mod corpus;
pub mod depth;
pub mod encode_diff;
pub mod encoding;
pub mod exact;
pub mod grammar;
pub mod numbers;
pub mod outcome;
pub mod record;
pub mod status;
pub mod strings;
pub mod structs;
pub mod value_targets;

use std::path::PathBuf;

use codecs::Backend;
use serde_json::{Map, Value, json};

use crate::corpus::Corpus;
use crate::record::Section;

/// Version of the report's shape.
pub const SCHEMA: u32 = 1;

/// Stack for the thread a backend's suite runs on: deep enough that the
/// suite's own recursion (serializing and comparing `nested-10000`, and any
/// backend recursing through `n_structure_100000_opening_arrays`) is never
/// what overflows. Reserved, not committed.
pub const SUITE_STACK: usize = 512 << 20;

/// What one run needs besides the backend.
#[derive(Debug, Clone)]
pub struct Suite {
    corpus: Corpus,
    probe: Option<PathBuf>,
    float_samples: usize,
}

impl Suite {
    /// A suite over `corpus`, with no depth prober and
    /// [`numbers::SAMPLES`] random floats.
    #[must_use]
    pub fn new(corpus: Corpus) -> Self {
        Self {
            corpus,
            probe: None,
            float_samples: numbers::SAMPLES,
        }
    }

    /// Run depth probes by re-executing `exe` (the `conform` binary).
    #[must_use]
    pub fn with_probe(mut self, exe: PathBuf) -> Self {
        self.probe = Some(exe);
        self
    }

    /// Check `samples` random floats instead of [`numbers::SAMPLES`].
    #[must_use]
    pub fn with_float_samples(mut self, samples: usize) -> Self {
        self.float_samples = samples;
        self
    }

    /// The verified corpus.
    #[must_use]
    pub fn corpus(&self) -> &Corpus {
        &self.corpus
    }

    /// Every section for `B`, as the report's backend entry.
    ///
    /// Runs on a thread with a [`SUITE_STACK`] stack, so it is safe to call
    /// from a test thread.
    #[must_use]
    pub fn run_backend<B: Backend>(&self) -> Value {
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name(format!("conform-{}", B::NAME))
                .stack_size(SUITE_STACK)
                .spawn_scoped(scope, || record::backend_entry(self.sections::<B>()))
                .map_or_else(
                    |error| crashed(&format!("could not spawn the suite thread: {error}")),
                    |handle| handle.join().unwrap_or_else(|_| crashed("the suite itself panicked")),
                )
        })
    }

    fn sections<B: Backend>(&self) -> Vec<Section> {
        vec![
            grammar::run::<B>(&self.corpus),
            status::run::<B>(&self.corpus),
            strings::run::<B>(),
            structs::run::<B>(),
            depth::run::<B>(self.probe.as_deref()),
            numbers::run::<B>(self.float_samples),
            attributes::run::<B>(),
            value_targets::run::<B>(&self.corpus),
            encoding::run::<B>(&self.corpus),
            encode_diff::run::<B>(),
        ]
    }
}

/// A backend entry for a suite that did not finish.
#[must_use]
pub fn crashed(detail: &str) -> Value {
    json!({
        "gate": "fail",
        "gating_failures": [{ "section": "process", "case": "suite", "detail": detail }],
        "sections": {},
    })
}

/// Target features this binary was compiled with that change a backend's
/// code path.
#[must_use]
pub fn target_features() -> Vec<&'static str> {
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

/// The report around `backends`, each keyed by name.
#[must_use]
pub fn report(backends: Map<String, Value>) -> Value {
    let mut report = json!({
        "schema": SCHEMA,
        "reference": codecs::backend::serde_json::SerdeJson::NAME,
        "serde_json_features": {
            "float_roundtrip": cfg!(feature = "float-roundtrip"),
            "arbitrary_precision": cfg!(feature = "arbitrary-precision"),
        },
        "target_features": target_features(),
    });
    report["backends"] = Value::Object(backends);
    report
}

/// The report for a run that could not start, such as a corpus that failed
/// verification.
#[must_use]
pub fn error_report(error: &str) -> Value {
    let mut report = report(Map::new());
    report["error"] = json!(error);
    report
}
