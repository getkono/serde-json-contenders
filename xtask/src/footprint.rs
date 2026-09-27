//! What a backend brings into a build: its crates, their `unsafe`, and their
//! advisories.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use syn::visit::Visit;

use crate::counted::Filter;
use crate::matrix::Set;
use crate::util::{output, root, run};

/// The RustSec advisory database commit the advisory check reads.
pub const ADVISORY_DB_COMMIT: &str = "e2111519ba6d14a5da59a7b2e5c8083ae8a37c01";

/// `name version` of every normal dependency `codecs` resolves in `set`.
fn tree(set: &Set) -> Result<BTreeSet<(String, String)>> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root()).args([
        "tree", "--locked", "-p", "codecs", "-e", "normal", "--prefix", "none", "--format", "{p}",
    ]);
    if !set.features.is_empty() {
        cmd.args(["--features", set.features]);
    }
    Ok(output(&mut cmd)?
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            Some((
                parts.next()?.to_owned(),
                parts.next()?.trim_start_matches('v').to_owned(),
            ))
        })
        .filter(|(name, _)| name != "codecs")
        .collect())
}

/// Crates in `codecs`' tree that are not JSON: the adapter's own needs.
const HARNESS: &[&str] = &[
    "bytes",
    "serde",
    "serde_core",
    "serde_derive",
    "proc-macro2",
    "quote",
    "syn",
    "unicode-ident",
];

/// Counts of `unsafe` items in one crate's source.
#[derive(Debug, Default, Clone, Copy)]
struct Unsafe {
    blocks: u64,
    fns: u64,
    impls: u64,
    traits: u64,
}

impl<'ast> Visit<'ast> for Unsafe {
    fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
        self.blocks += 1;
        syn::visit::visit_expr_unsafe(self, node);
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if node.sig.unsafety.is_some() {
            self.fns += 1;
        }
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if node.sig.unsafety.is_some() {
            self.fns += 1;
        }
        syn::visit::visit_impl_item_fn(self, node);
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if node.unsafety.is_some() {
            self.impls += 1;
        }
        syn::visit::visit_item_impl(self, node);
    }
    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if node.unsafety.is_some() {
            self.traits += 1;
        }
        syn::visit::visit_item_trait(self, node);
    }
}

fn count_unsafe(dir: &std::path::Path) -> (Unsafe, u64, u64) {
    let mut total = Unsafe::default();
    let (mut files, mut unparsed) = (0, 0);
    for entry in walkdir::WalkDir::new(dir.join("src"))
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.path().extension().is_some_and(|e| e == "rs") {
            files += 1;
            match std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|t| syn::parse_file(&t).ok())
            {
                Some(file) => total.visit_file(&file),
                None => unparsed += 1,
            }
        }
    }
    (total, files, unparsed)
}

/// Crates that exist only to build proc macros.
const MACRO_SUPPORT: &[&str] = &["syn", "quote", "proc-macro2", "unicode-ident"];

/// A package's source directory, and whether it only runs at build time (a
/// proc macro, or a crate proc macros are built from).
struct Source {
    dir: std::path::PathBuf,
    build_only: bool,
}

/// Package sources for `set`, from `cargo metadata` (which lists only the
/// optional dependencies the requested features activate).
fn sources(set: &Set) -> Result<BTreeMap<(String, String), Source>> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root())
        .args(["metadata", "--format-version", "1", "--locked"]);
    if !set.features.is_empty() {
        cmd.args(["--features", set.features]);
    }
    let meta: Value = serde_json::from_str(&output(&mut cmd)?)?;
    Ok(meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let dir = std::path::Path::new(p["manifest_path"].as_str()?)
                .parent()?
                .to_path_buf();
            let name = p["name"].as_str()?.to_owned();
            let proc_macro = p["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|t| t["kind"].as_array().into_iter().flatten().any(|k| k == "proc-macro"));
            let build_only = proc_macro || MACRO_SUPPORT.contains(&name.as_str());
            Some(((name, p["version"].as_str()?.to_owned()), Source { dir, build_only }))
        })
        .collect())
}

/// Ensure the advisory database is checked out at the pinned commit.
fn advisory_db() -> Result<std::path::PathBuf> {
    let dir = root().join("target/advisory-db");
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(&dir)?;
        run(Command::new("git").current_dir(&dir).args(["init", "-q"]))?;
        run(Command::new("git").current_dir(&dir).args([
            "remote",
            "add",
            "origin",
            "https://github.com/rustsec/advisory-db",
        ]))?;
    }
    run(Command::new("git")
        .current_dir(&dir)
        .args(["fetch", "-q", "--depth", "1", "origin", ADVISORY_DB_COMMIT]))?;
    run(Command::new("git")
        .current_dir(&dir)
        .args(["checkout", "-q", "FETCH_HEAD"]))?;
    Ok(dir)
}

/// Advisories that name `name`, with whether `version` is affected.
fn advisories(db: &std::path::Path, name: &str, version: &str) -> Result<Vec<Value>> {
    let dir = db.join("crates").join(name);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let version = semver::Version::parse(version)?;
    let mut found = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let text = std::fs::read_to_string(entry.path())?;
        let Some(front) = text.split("```toml").nth(1).and_then(|t| t.split("```").next()) else {
            continue;
        };
        let doc: toml::Value = toml::from_str(front).with_context(|| format!("parsing {}", entry.path().display()))?;
        let reqs = |key: &str| -> Vec<semver::VersionReq> {
            doc.get("versions")
                .and_then(|v| v.get(key))
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_str().and_then(|r| semver::VersionReq::parse(r).ok()))
                .collect()
        };
        let safe = reqs("patched")
            .iter()
            .chain(reqs("unaffected").iter())
            .any(|r| r.matches(&version));
        let advisory = doc.get("advisory");
        let get = |key: &str| {
            advisory
                .and_then(|a| a.get(key))
                .map(|v| v.to_string().trim_matches('"').to_owned())
        };
        found.push(json!({
            "id": get("id"),
            "informational": get("informational"),
            "withdrawn": get("withdrawn"),
            "title": get("title"),
            "affects_pinned_version": !safe && get("withdrawn").is_none(),
        }));
    }
    Ok(found)
}

/// `xtask footprint`: per set, the crates it adds, their `unsafe` counts, and
/// their advisories at the pinned database commit.
pub fn footprint(args: &[String]) -> Result<()> {
    let filter = Filter::parse(args);
    let db = advisory_db()?;
    let base = tree(crate::matrix::set("baseline")?)?;
    let mut rows = Vec::new();
    for set in filter.sets() {
        let sources = sources(set)?;
        let full = tree(set)?;
        let added: BTreeSet<_> = if set.name.starts_with("baseline") {
            full.iter()
                .filter(|(n, _)| !HARNESS.contains(&n.as_str()))
                .cloned()
                .collect()
        } else {
            full.difference(&base).cloned().collect()
        };
        let mut crates = Vec::new();
        let mut total = Unsafe::default();
        for (name, version) in &added {
            let source = sources.get(&(name.clone(), version.clone()));
            let (count, files, unparsed) = source.map_or((Unsafe::default(), 0, 0), |s| count_unsafe(&s.dir));
            let build_only = source.is_some_and(|s| s.build_only);
            anyhow::ensure!(source.is_some() && files > 0, "no sources found for {name} {version}");
            if !build_only {
                total.blocks += count.blocks;
                total.fns += count.fns;
                total.impls += count.impls;
                total.traits += count.traits;
            }
            crates.push(json!({
                "name": name, "version": version, "build_only": build_only,
                "unsafe_blocks": count.blocks, "unsafe_fns": count.fns, "unsafe_impls": count.impls, "unsafe_traits": count.traits,
                "files": files, "unparsed_files": unparsed,
                "advisories": advisories(&db, name, version)?,
            }));
        }
        rows.push(json!({
            "set": set.name, "crate": set.krate, "crates_added": added.len(),
            "runtime_crates_added": crates.iter().filter(|c| c["build_only"] == false).count(),
            "unsafe_blocks": total.blocks, "unsafe_fns": total.fns, "unsafe_impls": total.impls, "unsafe_traits": total.traits,
            "crates": crates,
        }));
    }
    crate::util::merge_write(
        &root().join("results/footprint.json"),
        "footprint",
        &rows,
        &["set"],
        json!({ "advisory_db": ADVISORY_DB_COMMIT }),
    )
}
