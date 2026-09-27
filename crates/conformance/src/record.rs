//! What a section found, in the report's shape.
//!
//! A *disagreement* is a case where the backend and serde_json did not agree.
//! A *gating failure* is a disagreement (or a spec violation) that fails the
//! backend's gate. Both are counted exactly; only their listings are capped.

use serde_json::{Map, Value, json};

/// Listed `differences` per section; `disagreements` is always the true count.
pub const DIFFERENCE_CAP: usize = 50;
/// Listed gating failures per section; `gating_failure_count` is the true count.
pub const FAILURE_CAP: usize = 100;

/// One section's findings for one backend.
#[derive(Debug)]
pub struct Section {
    name: &'static str,
    cases: usize,
    disagreements: usize,
    notes: Vec<String>,
    differences: Vec<Value>,
    failures: Vec<Value>,
    failure_count: usize,
    data: Map<String, Value>,
}

impl Section {
    /// An empty section called `name`.
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            cases: 0,
            disagreements: 0,
            notes: Vec::new(),
            differences: Vec::new(),
            failures: Vec::new(),
            failure_count: 0,
            data: Map::new(),
        }
    }

    /// The section's name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Count one case.
    pub fn case(&mut self) {
        self.cases += 1;
    }

    /// Record a disagreement with serde_json.
    pub fn differ(&mut self, case: &str, reference: Value, backend: Value) {
        self.disagreements += 1;
        if self.differences.len() < DIFFERENCE_CAP {
            let mut difference = Map::new();
            difference.insert("case".to_owned(), Value::String(case.to_owned()));
            difference.insert("reference".to_owned(), reference);
            difference.insert("backend".to_owned(), backend);
            self.differences.push(Value::Object(difference));
        }
    }

    /// Record a gating failure.
    pub fn fail(&mut self, case: &str, detail: impl Into<String>) {
        self.failure_count += 1;
        if self.failures.len() < FAILURE_CAP {
            self.failures
                .push(json!({ "section": self.name, "case": case, "detail": detail.into() }));
        }
    }

    /// Record a non-gating observation.
    pub fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    /// Attach structured, section-specific data.
    pub fn data(&mut self, key: &str, value: Value) {
        self.data.insert(key.to_owned(), value);
    }

    /// Gating failures recorded so far.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.failure_count
    }

    /// The section object, and its listed gating failures.
    #[must_use]
    pub fn finish(self) -> (Value, Vec<Value>) {
        let mut section = json!({
            "cases": self.cases,
            "disagreements": self.disagreements,
            "gating": self.failure_count > 0,
            "gating_failure_count": self.failure_count,
            "notes": self.notes,
            "differences": self.differences,
        });
        if !self.data.is_empty() {
            section["data"] = Value::Object(self.data);
        }
        (section, self.failures)
    }
}

/// Assemble a backend's entry from its sections.
#[must_use]
pub fn backend_entry(sections: Vec<Section>) -> Value {
    let mut failures = Vec::new();
    let mut by_name = Map::new();
    for section in sections {
        let name = section.name();
        let (value, listed) = section.finish();
        failures.extend(listed);
        by_name.insert(name.to_owned(), value);
    }
    json!({
        "gate": if failures.is_empty() { "pass" } else { "fail" },
        "gating_failures": failures,
        "sections": by_name,
    })
}
