//! The fixture's copied body types read and write exactly what the
//! `payloads` originals do.

use payloads::Workload;
use payloads::kynos::{EchoPost, JsonLarge, JsonSmall};
use serde::Serialize;
use serde::de::DeserializeOwned;
use size_fixture::body::{Catalog, CatalogRef, Echo, EchoRef, Item, ItemRef};

/// Decode `W`'s bytes into the copy `T` and re-encode them: the same bytes,
/// which are also what the original encodes to.
fn same_bytes<W: Workload, T: Serialize + DeserializeOwned>() -> serde_json::Result<()> {
    let bytes = W::bytes();
    assert_eq!(serde_json::to_vec(&W::value())?, bytes, "{}", W::NAME);
    assert_eq!(
        serde_json::to_vec(&serde_json::from_slice::<T>(&bytes)?)?,
        bytes,
        "{}",
        W::NAME
    );
    Ok(())
}

#[test]
fn owned_copies_match_the_originals() -> serde_json::Result<()> {
    same_bytes::<JsonSmall, Item>()?;
    same_bytes::<EchoPost, Echo>()?;
    same_bytes::<JsonLarge, Catalog>()
}

#[test]
fn borrowed_copies_match_the_originals() -> serde_json::Result<()> {
    let small = JsonSmall::bytes();
    assert_eq!(
        serde_json::to_vec(&serde_json::from_slice::<ItemRef<'_>>(&small)?)?,
        small
    );
    let echo = EchoPost::bytes();
    assert_eq!(
        serde_json::to_vec(&serde_json::from_slice::<EchoRef<'_>>(&echo)?)?,
        echo
    );
    let large = JsonLarge::bytes();
    assert_eq!(
        serde_json::to_vec(&serde_json::from_slice::<CatalogRef<'_>>(&large)?)?,
        large
    );
    Ok(())
}

#[test]
fn the_fixture_decodes_both_bodies() {
    let echo = EchoPost::value().payload.len() as u8;
    assert_eq!(size_fixture::round_trip(EchoPost::bytes().into(), false), [echo]);
    let catalog = JsonLarge::value().items.len() as u8;
    assert_eq!(size_fixture::round_trip(JsonLarge::bytes().into(), false), [catalog]);
}
