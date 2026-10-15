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
///
/// Parsed code is counted from the syntax tree. A macro's body (a
/// `macro_rules!` definition, or the arguments of any invocation) is only
/// tokens to the parser, so there every `unsafe` keyword token is counted
/// and classed by the token after it: a `{` group is a block, `fn` or
/// `extern` a function, `impl` an impl and `trait` a trait; a `(` group is an
/// unsafe attribute (`#[unsafe(no_mangle)]`), which is not counted outside
/// macros either; anything else (a metavariable such as `unsafe $body`) is
/// counted as a block, the only other thing `unsafe` can introduce. A
/// `macro_rules!` body is counted once however many times it expands.
/// `in_macros` is how many of the four counts came from macro tokens.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Unsafe {
    blocks: u64,
    fns: u64,
    impls: u64,
    traits: u64,
    in_macros: u64,
}

impl Unsafe {
    fn add(&mut self, other: Self) {
        self.blocks += other.blocks;
        self.fns += other.fns;
        self.impls += other.impls;
        self.traits += other.traits;
        self.in_macros += other.in_macros;
    }

    /// Count the `unsafe` keyword tokens in a macro body, at any depth.
    fn tokens(&mut self, input: syn::parse::ParseStream<'_>) -> syn::Result<()> {
        use syn::{Token, token};
        while !input.is_empty() {
            if input.peek(Token![unsafe]) {
                input.parse::<Token![unsafe]>()?;
                if input.peek(token::Paren) {
                    continue;
                }
                self.in_macros += 1;
                if input.peek(Token![fn]) || input.peek(Token![extern]) {
                    self.fns += 1;
                } else if input.peek(Token![impl]) {
                    self.impls += 1;
                } else if input.peek(Token![trait]) {
                    self.traits += 1;
                } else {
                    self.blocks += 1;
                }
            } else if input.peek(token::Brace) {
                let inner;
                syn::braced!(inner in input);
                self.tokens(&inner)?;
            } else if input.peek(token::Paren) {
                let inner;
                syn::parenthesized!(inner in input);
                self.tokens(&inner)?;
            } else if input.peek(token::Bracket) {
                let inner;
                syn::bracketed!(inner in input);
                self.tokens(&inner)?;
            } else {
                input.step(|cursor| {
                    cursor
                        .token_tree()
                        .map(|(_, rest)| ((), rest))
                        .ok_or_else(|| cursor.error("unexpected end of macro body"))
                })?;
            }
        }
        Ok(())
    }
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
    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if node.sig.unsafety.is_some() {
            self.fns += 1;
        }
        syn::visit::visit_trait_item_fn(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // Token streams always parse as tokens; a failure would only mean
        // a malformed group, and then what was counted before it stands.
        let mut found = Self::default();
        let _ = syn::parse::Parser::parse2(
            |input: syn::parse::ParseStream<'_>| found.tokens(input),
            node.tokens.clone(),
        );
        self.add(found);
        syn::visit::visit_macro(self, node);
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

/// Top-level package directories whose code never reaches a dependent's
/// build: build output and the package's own tests, benchmarks and examples.
const NOT_SHIPPED: &[&str] = &["target", "tests", "benches", "examples"];

/// The `unsafe` in every `.rs` file of the package at `dir`, except under
/// its top-level [`NOT_SHIPPED`] directories: `src/`, a build script, and
/// any other directory a target or `#[path]` may compile from. With the
/// number of files read, and of files that did not parse.
fn count_unsafe(dir: &std::path::Path) -> (Unsafe, u64, u64) {
    let mut total = Unsafe::default();
    let (mut files, mut unparsed) = (0, 0);
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            !(e.depth() == 1 && e.file_type().is_dir() && NOT_SHIPPED.iter().any(|n| e.file_name() == *n))
        })
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|e| e == "rs") {
            files += 1;
            match std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|t| count_source(&t))
            {
                Some(count) => total.add(count),
                None => unparsed += 1,
            }
        }
    }
    (total, files, unparsed)
}

/// The `unsafe` in one file's text, if it parses.
fn count_source(text: &str) -> Option<Unsafe> {
    let mut count = Unsafe::default();
    count.visit_file(&syn::parse_file(text).ok()?);
    Some(count)
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
                total.add(count);
            }
            crates.push(json!({
                "name": name, "version": version, "build_only": build_only,
                "unsafe_blocks": count.blocks, "unsafe_fns": count.fns, "unsafe_impls": count.impls, "unsafe_traits": count.traits,
                "unsafe_in_macros": count.in_macros,
                "files": files, "unparsed_files": unparsed,
                "advisories": advisories(&db, name, version)?,
            }));
        }
        rows.push(json!({
            "set": set.name, "crate": set.krate, "crates_added": added.len(),
            "runtime_crates_added": crates.iter().filter(|c| c["build_only"] == false).count(),
            "unsafe_blocks": total.blocks, "unsafe_fns": total.fns, "unsafe_impls": total.impls, "unsafe_traits": total.traits,
            "unsafe_in_macros": total.in_macros,
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

#[cfg(test)]
mod tests {
    use super::{Unsafe, count_source, count_unsafe};

    fn count(text: &str) -> Unsafe {
        count_source(text).unwrap_or_else(|| panic!("does not parse: {text}"))
    }

    #[test]
    fn items_and_blocks_are_counted_from_the_tree() {
        let found = count(
            "unsafe fn f() { unsafe { g() } }
             struct S;
             impl S { unsafe fn m(&self) {} }
             unsafe trait T { unsafe fn declared(); unsafe fn defaulted() {} fn safe(); }
             unsafe impl T for S { unsafe fn declared() {} }",
        );
        assert_eq!(
            found,
            Unsafe {
                blocks: 1,
                fns: 5,
                impls: 1,
                traits: 1,
                in_macros: 0
            }
        );
    }

    #[test]
    fn unsafe_inside_macros_is_counted_from_tokens() {
        let found = count(
            r#"macro_rules! m {
                 ($body:block) => { unsafe $body };
                 ($t:ty) => { unsafe impl Send for $t {} pub unsafe fn f() { unsafe { g() } } unsafe extern "C" fn h() {} };
               }
               fn f() { assert!(unsafe { g() }); let v = vec![unsafe { g() }]; }
               #[unsafe(no_mangle)] extern "C" fn n() {}
               m! { #[unsafe(no_mangle)] fn k() {} }"#,
        );
        assert_eq!(
            found,
            Unsafe {
                blocks: 4,
                fns: 2,
                impls: 1,
                traits: 0,
                in_macros: 7
            }
        );
    }

    #[test]
    fn the_word_in_strings_comments_and_raw_identifiers_is_not_unsafe() {
        let found = count(
            r#"// unsafe { }
               fn f() { let r#unsafe = "unsafe { }"; println!("unsafe {}", r#unsafe); }"#,
        );
        assert_eq!(found, Unsafe::default());
    }

    #[test]
    fn the_whole_package_is_read_but_tests_benches_examples_and_target() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("xtask-footprint-{}", std::process::id()));
        let write = |path: &str, text: &str| -> std::io::Result<()> {
            let path = dir.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, text)
        };
        let one = "fn f() { unsafe {} }";
        write("src/lib.rs", one)?;
        write("src/tests/inner.rs", one)?;
        write("build.rs", one)?;
        write("arch/x86.rs", one)?;
        write("broken.rs", "fn (")?;
        write("tests/t.rs", one)?;
        write("benches/b.rs", one)?;
        write("examples/e.rs", one)?;
        write("target/debug/build/out.rs", one)?;
        let (found, files, unparsed) = count_unsafe(&dir);
        std::fs::remove_dir_all(&dir)?;
        assert_eq!((found.blocks, files, unparsed), (4, 5, 1));
        Ok(())
    }
}
