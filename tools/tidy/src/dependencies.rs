//! Declared dependencies that no source of their package names.
//!
//! A conservative text check in the manner of `cargo machete`: a dependency
//! counts as used when its crate name (the key with `-` as `_`, or a path
//! package's library name) appears as an identifier in any file the
//! package's crate roots reach, including the build script and
//! `include_str!` documents, or when a feature forwards to it
//! (`dependency/feature`). Crates linked only for their side effects need a
//! reviewed allowlist entry.

use std::collections::BTreeSet;

use crate::{Context, Result, reachability};

/// Every declared dependency is named, and every allowlist entry still
/// excuses a declared, otherwise unused dependency.
pub fn check(context: &Context<'_>) -> Result<Vec<String>> {
    let mut problems = vec![];
    let mut excused = BTreeSet::new();
    for package in context.model.packages() {
        let Some(reach) = context.reach.get(&package.manifest) else {
            continue;
        };
        let mut names = BTreeSet::new();
        for file in &reach.files {
            reachability::names(&context.repo.read(file)?, &mut names);
        }
        for file in &reach.texts {
            reachability::words(&context.repo.read(file)?, &mut names);
        }
        for dependency in &package.dependencies {
            let mut crate_names = vec![dependency.key.replace('-', "_")];
            if let Some(target) = dependency
                .path
                .as_deref()
                .and_then(|path| context.model.package_at(path))
            {
                crate_names.push(target.library.clone());
            }
            let used = crate_names.iter().any(|name| names.contains(name))
                || package.feature_forwarded.contains(&dependency.key);
            let allowed = context
                .allowlist
                .unused_dependencies
                .iter()
                .position(|entry| {
                    entry.manifest == package.manifest && entry.dependency == dependency.key
                });
            match (used, allowed) {
                (false, None) => problems.push(format!(
                    "{}: [{}] {} is named by no source of {}",
                    package.manifest, dependency.table, dependency.key, package.name
                )),
                (true, Some(_)) => problems.push(format!(
                    "{}: {} is allowlisted as unused but its sources name it",
                    package.manifest, dependency.key
                )),
                (false, Some(index)) => {
                    excused.insert(index);
                }
                (true, None) => {}
            }
        }
    }
    for (index, entry) in context.allowlist.unused_dependencies.iter().enumerate() {
        let declared = context.model.packages().iter().any(|package| {
            package.manifest == entry.manifest
                && package
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.key == entry.dependency)
        });
        if !declared && !excused.contains(&index) {
            problems.push(format!(
                "{}: {} is allowlisted as unused but not declared",
                entry.manifest, entry.dependency
            ));
        }
    }
    problems.sort();
    problems.dedup();
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    fn run(manifest: &str, lib: &str, extra: &[(&str, &str)]) -> Vec<String> {
        let mut files = vec![("p/Cargo.toml", manifest), ("p/src/lib.rs", lib)];
        files.extend_from_slice(extra);
        problems(&files, |context| check(context).unwrap())
    }

    #[test]
    fn named_renamed_and_forwarded_dependencies_pass() {
        let manifest = "[package]\nname = \"p\"\n[features]\nstd = [\"serde-json/std\"]\n\
                        [dependencies]\nserde-json = \"1\"\nfast_hash = { package = \"ahash\", version = \"1\" }\n\
                        facade = { path = \"../facade\" }\n[build-dependencies]\ncc = \"1\"\n\
                        [dev-dependencies]\ntempfile = \"3\"\n";
        let found = run(
            manifest,
            "use fast_hash::Map;\nuse renamed_lib::Thing;\n#[cfg(test)] mod tests;\n",
            &[
                ("p/build.rs", "fn main() { cc::Build::new(); }"),
                ("p/src/tests.rs", "fn t() { tempfile::tempdir(); }"),
                (
                    "facade/Cargo.toml",
                    "[package]\nname = \"facade\"\n[lib]\nname = \"renamed_lib\"\n",
                ),
                ("facade/src/lib.rs", ""),
            ],
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn an_unnamed_dependency_fails_even_when_a_comment_mentions_it() {
        let manifest =
            "[package]\nname = \"p\"\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n";
        let found = run(
            manifest,
            "// libc was used here\n/* libc */\npub fn f() {}\n",
            &[],
        );
        assert_eq!(
            found,
            ["p/Cargo.toml: [target.'cfg(unix)'.dependencies] libc is named by no source of p"]
        );
        let found = run(
            manifest,
            "/// ```\n/// libc::getpid();\n/// ```\npub fn f() {}\n",
            &[],
        );
        assert!(found.is_empty(), "a doctest names it: {found:?}");
        let found = run(
            manifest,
            "const URL: &str = \"http://x\"; use libc::c_int;\n",
            &[],
        );
        assert!(
            found.is_empty(),
            "a string does not open a comment: {found:?}"
        );
    }

    #[test]
    fn allowlist_entries_must_still_excuse_something() {
        let allowlist = "[[unused-dependency]]\nmanifest = \"p/Cargo.toml\"\ndependency = \"panic-halt\"\nreason = \"panic handler\"\n\
                         [[unused-dependency]]\nmanifest = \"p/Cargo.toml\"\ndependency = \"used\"\nreason = \"stale\"\n\
                         [[unused-dependency]]\nmanifest = \"p/Cargo.toml\"\ndependency = \"removed\"\nreason = \"stale\"\n";
        let found = run(
            "[package]\nname = \"p\"\n[dependencies]\npanic-halt = \"1\"\nused = \"1\"\n",
            "use used as _;\n",
            &[("tools/tidy/allowlist.toml", allowlist)],
        );
        assert_eq!(
            found,
            [
                "p/Cargo.toml: removed is allowlisted as unused but not declared",
                "p/Cargo.toml: used is allowlisted as unused but its sources name it",
            ]
        );
    }
}
