//! The vendored JSONTestSuite `test_parsing/` corpus, verified before use.
//!
//! A conformance claim is only as good as its inputs, so the corpus is
//! checked against `SHA256SUMS` and its expected counts; any drift aborts
//! the run rather than quietly testing something else.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Expected `y_` / `n_` / `i_` counts, per `references/JSONTestSuite/README.md`.
pub const EXPECTED: (usize, usize, usize) = (95, 188, 35);

/// What RFC 8259 requires of a parser for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// `y_`: must accept.
    Accept,
    /// `n_`: must reject.
    Reject,
    /// `i_`: implementation-defined.
    Either,
}

/// One file.
#[derive(Debug, Clone)]
pub struct Case {
    /// The file name.
    pub name: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
    /// What a parser must do with it.
    pub expect: Expect,
}

/// Every file, sorted by name.
#[derive(Debug, Clone)]
pub struct Corpus {
    /// The cases.
    pub cases: Vec<Case>,
}

/// The vendored corpus directory.
#[must_use]
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../references/JSONTestSuite")
}

impl Corpus {
    /// Load `dir`, verifying every digest in `SHA256SUMS`, that it lists
    /// exactly the files present, and the expected counts.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let corpus = Self::load_unverified(dir)?;
        let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).map_err(|e| format!("SHA256SUMS: {e}"))?;
        let mut listed = BTreeSet::new();
        for line in sums.lines().filter(|line| !line.trim().is_empty()) {
            let (digest, name) = line
                .split_once("  ")
                .ok_or_else(|| format!("SHA256SUMS: malformed line {line:?}"))?;
            listed.insert(name.to_owned());
            let case = corpus
                .cases
                .iter()
                .find(|case| case.name == name)
                .ok_or_else(|| format!("{name}: listed in SHA256SUMS but missing"))?;
            let actual = hex::encode(Sha256::digest(&case.bytes));
            if actual != digest {
                return Err(format!("{name}: SHA-256 {actual}, SHA256SUMS says {digest}"));
            }
        }
        let present: BTreeSet<String> = corpus.cases.iter().map(|case| case.name.clone()).collect();
        if let Some(extra) = present.difference(&listed).next() {
            return Err(format!("{extra}: present but not listed in SHA256SUMS"));
        }
        Ok(corpus)
    }

    /// Load `dir` checking only the counts: for Miri, which cannot run the
    /// SHA-256 implementation's CPU feature detection.
    pub fn load_unverified(dir: &Path) -> Result<Self, String> {
        let parsing = dir.join("test_parsing");
        let entries = std::fs::read_dir(&parsing).map_err(|e| format!("{}: {e}", parsing.display()))?;
        let mut cases = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let expect = match name.get(..2) {
                Some("y_") => Expect::Accept,
                Some("n_") => Expect::Reject,
                Some("i_") => Expect::Either,
                _ => return Err(format!("{name}: not a y_/n_/i_ file")),
            };
            let bytes = std::fs::read(entry.path()).map_err(|e| format!("{name}: {e}"))?;
            cases.push(Case { name, bytes, expect });
        }
        cases.sort_by(|a, b| a.name.cmp(&b.name));
        let count = |expect| cases.iter().filter(|case| case.expect == expect).count();
        let counts = (count(Expect::Accept), count(Expect::Reject), count(Expect::Either));
        if counts != EXPECTED {
            return Err(format!("y_/n_/i_ counts are {counts:?}, expected {EXPECTED:?}"));
        }
        Ok(Self { cases })
    }

    /// The cases expecting `expect`.
    pub fn expecting(&self, expect: Expect) -> impl Iterator<Item = &Case> {
        self.cases.iter().filter(move |case| case.expect == expect)
    }
}
