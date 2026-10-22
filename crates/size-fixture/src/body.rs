//! The Kynos body types the fixture declares, copied from `payloads::kynos`
//! with the same fields, order and serde attributes.
//!
//! A copy rather than a dependency: `payloads` derives every workload's
//! types, and building them all would add the same harness compile time to
//! every set's clean build, diluting the ratios `xtask compile-time` reports.
//! `tests/body.rs` holds the copies to the originals' encoding.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// One record: the element of [`Catalog`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Item {
    /// Stable identifier.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Labels.
    pub tags: Vec<String>,
    /// Liveness.
    pub active: bool,
    /// A two-decimal weight.
    pub score: f64,
}

/// [`Item`], borrowing.
#[derive(Debug, Serialize, Deserialize)]
pub struct ItemRef<'a> {
    /// Stable identifier.
    pub id: u64,
    /// Display name.
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    /// Labels.
    #[serde(borrow)]
    pub tags: Vec<Cow<'a, str>>,
    /// Liveness.
    pub active: bool,
    /// A two-decimal weight.
    pub score: f64,
}

/// The `json-large` body.
#[derive(Debug, Serialize, Deserialize)]
pub struct Catalog {
    /// Fixture revision.
    pub revision: u32,
    /// Total records available.
    pub total: u64,
    /// The records.
    pub items: Vec<Item>,
}

/// [`Catalog`], borrowing.
#[derive(Debug, Serialize, Deserialize)]
pub struct CatalogRef<'a> {
    /// Fixture revision.
    pub revision: u32,
    /// Total records available.
    pub total: u64,
    /// The records.
    #[serde(borrow)]
    pub items: Vec<ItemRef<'a>>,
}

/// The `echo-post` body, in both directions.
#[derive(Debug, Serialize, Deserialize)]
pub struct Echo {
    /// Correlation id.
    pub id: u64,
    /// The bulk of the body.
    pub payload: String,
    /// Checksum of the payload bytes.
    pub checksum: u64,
}

/// [`Echo`], borrowing.
#[derive(Debug, Serialize, Deserialize)]
pub struct EchoRef<'a> {
    /// Correlation id.
    pub id: u64,
    /// The bulk of the body.
    #[serde(borrow)]
    pub payload: Cow<'a, str>,
    /// Checksum of the payload bytes.
    pub checksum: u64,
}
