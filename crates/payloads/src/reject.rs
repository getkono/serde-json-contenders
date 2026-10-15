//! Bodies a server must refuse: what turning a bad request away costs.
//!
//! Both are `json-large` damaged in one place, decoded into its types, so
//! the work before the damage is the work a valid body costs and the
//! difference is where the decoder stops. Their bytes encode no value, so
//! they have no encode: [`Workload::value`] is the value they were damaged
//! from.

use crate::kynos::{CATALOG_ITEMS, Catalog, CatalogRef, JsonLarge};
use crate::{Expect, Group, Workload};

/// `json-large`'s bytes cut at nine tenths of their length: the body ends
/// partway through the items, so the decoder reads 90 % of a valid body
/// before it can refuse. End of input, 400.
#[derive(Debug)]
pub struct RejectTruncated;

impl Workload for RejectTruncated {
    const NAME: &'static str = "reject-truncated";
    const GROUP: Group = Group::Family;
    const EXPECT: Expect = Expect::Reject(400);
    type Owned = Catalog;
    type Borrowed<'de> = CatalogRef<'de>;

    fn value() -> Catalog {
        JsonLarge::value()
    }

    fn bytes() -> Vec<u8> {
        let mut bytes = JsonLarge::bytes();
        bytes.truncate(bytes.len() * 9 / 10);
        bytes
    }
}

/// `json-large`'s bytes with the last item's `id` written as a string: valid
/// JSON of the wrong shape, found after every other item has been decoded.
/// Data, 422.
#[derive(Debug)]
pub struct RejectShape;

impl Workload for RejectShape {
    const NAME: &'static str = "reject-shape";
    const GROUP: Group = Group::Family;
    const EXPECT: Expect = Expect::Reject(422);
    type Owned = Catalog;
    type Borrowed<'de> = CatalogRef<'de>;

    fn value() -> Catalog {
        JsonLarge::value()
    }

    fn bytes() -> Vec<u8> {
        let last = CATALOG_ITEMS - 1;
        let text = String::from_utf8(JsonLarge::bytes()).unwrap_or_else(|_| unreachable!("serde_json writes UTF-8"));
        let from = format!("{{\"id\":{last},");
        let at = text
            .rfind(&from)
            .unwrap_or_else(|| unreachable!("json-large ends with item {last}"));
        let mut out = String::with_capacity(text.len() + 2);
        out.push_str(&text[..at]);
        out.push_str(&format!("{{\"id\":\"{last}\","));
        out.push_str(&text[at + from.len()..]);
        out.into_bytes()
    }
}
