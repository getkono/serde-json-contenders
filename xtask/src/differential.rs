//! The conformance differential, run over the build matrix.

use std::process::Command;

use anyhow::Result;
use serde_json::json;

use crate::build::{Place, bin, build_with};
use crate::counted::Filter;
use crate::util::{capture, root, write_json};

/// `xtask conformance`: every set at every variant (SIMD paths differ by
/// variant), plus each set with serde_json's `arbitrary_precision` unified
/// in, at the portable variant.
pub fn conformance(args: &[String]) -> Result<()> {
    let filter = Filter::parse(args);
    let host = crate::provenance::host_slug();
    let mut failures = 0;
    for variant in filter.variants() {
        for set in filter.sets() {
            let precisions: &[Option<()>] = if variant.name == "portable" {
                &[None, Some(())]
            } else {
                &[None]
            };
            for &ap in precisions {
                let release = build_with(Place::Host, "conformance", set, &variant, ap)?;
                let name = if ap.is_some() {
                    format!("{}+arbitrary-precision", set.name)
                } else {
                    set.name.to_owned()
                };
                eprintln!("conformance: {} {name}", variant.name);
                let (_, stdout, stderr) = capture(&mut Command::new(bin(&release, "conform")))?;
                let report: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
                    failures += 1;
                    json!({ "error": format!("conform produced no report: {e}"), "stderr": stderr })
                });
                let path = root()
                    .join("results")
                    .join(&host)
                    .join("conformance")
                    .join(variant.name)
                    .join(format!("{name}.json"));
                write_json(
                    &path,
                    &json!({ "schema": 1, "provenance": crate::provenance::stamp(), "report": report }),
                )?;
            }
        }
    }
    anyhow::ensure!(failures == 0, "{failures} conformance runs produced no report");
    Ok(())
}
