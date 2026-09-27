//! The three shapes a Kynos server is benchmarked on, ported verbatim.
//!
//! Field order is serialization order and the bytes are pinned by digest, so
//! these types must not be reordered.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::{Group, Workload};

/// One record: the `json-small` body and the element of `json-large`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Fixture revision.
    pub revision: u32,
    /// Total records available.
    pub total: u64,
    /// The records.
    pub items: Vec<Item>,
}

/// [`Catalog`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Echo {
    /// Correlation id.
    pub id: u64,
    /// The bulk of the body.
    pub payload: String,
    /// Checksum of the payload bytes.
    pub checksum: u64,
}

/// [`Echo`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EchoRef<'a> {
    /// Correlation id.
    pub id: u64,
    /// The bulk of the body.
    #[serde(borrow)]
    pub payload: Cow<'a, str>,
    /// Checksum of the payload bytes.
    pub checksum: u64,
}

/// Records in `json-large`; lands the body near 64 KiB.
pub const CATALOG_ITEMS: u64 = 705;
/// Which record `json-small` serves; not zero, so fields do not collapse.
pub const SMALL_ITEM: u64 = 100;
/// `echo-post` payload length.
pub const ECHO_PAYLOAD: usize = 1024;

/// The record at `index`, a closed-form function of the index.
#[must_use]
pub fn item(index: u64) -> Item {
    Item {
        id: index,
        name: format!("item-{index:06}"),
        tags: vec![
            format!("tag-{}", index % 7),
            format!("tag-{}", index % 13),
            format!("tag-{}", index % 29),
        ],
        active: !index.is_multiple_of(3),
        score: f64::from(u32::try_from(index % 10_000).unwrap_or(0)) / 100.0,
    }
}

/// Additive checksum proving the payload was read.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(u64::from(*b)))
}

/// `json-small`: one [`Item`], ~91 bytes.
#[derive(Debug)]
pub struct JsonSmall;

impl Workload for JsonSmall {
    const NAME: &'static str = "json-small";
    const GROUP: Group = Group::Kynos;
    type Owned = Item;
    type Borrowed<'de> = ItemRef<'de>;

    fn value() -> Item {
        item(SMALL_ITEM)
    }
}

/// `echo-post`: a 1 KiB [`Echo`], decoded and re-encoded.
#[derive(Debug)]
pub struct EchoPost;

impl Workload for EchoPost {
    const NAME: &'static str = "echo-post";
    const GROUP: Group = Group::Kynos;
    type Owned = Echo;
    type Borrowed<'de> = EchoRef<'de>;

    fn value() -> Echo {
        let payload: String = (0..ECHO_PAYLOAD).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let checksum = checksum(payload.as_bytes());
        Echo {
            id: 7,
            payload,
            checksum,
        }
    }
}

/// `json-large`: a [`Catalog`] of 705 items, ~64 KiB.
#[derive(Debug)]
pub struct JsonLarge;

impl Workload for JsonLarge {
    const NAME: &'static str = "json-large";
    const GROUP: Group = Group::Kynos;
    type Owned = Catalog;
    type Borrowed<'de> = CatalogRef<'de>;

    fn value() -> Catalog {
        Catalog {
            revision: 1,
            total: 1_000_000,
            items: (0..CATALOG_ITEMS).map(item).collect(),
        }
    }
}
