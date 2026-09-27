//! One module per backend crate. A module compiles only when its crate's
//! feature is on; `serde_json` is always compiled, as the reference.

pub mod serde_json;
