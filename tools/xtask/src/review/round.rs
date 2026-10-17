//! Which review a push needs: the whole diff, or a fix round's.
//!
//! The first push of a branch gets the full review of its diff against the
//! merge base. A fix round, a push after an earlier one, gets the review of
//! what changed since the previous pushed head, with the findings of the
//! branch's previous verdict and every owner decision the pull request
//! implements; it is full again when the round's diff touches concurrency,
//! `unsafe`, the public items of a file (parsed, [`public_items`]) or a
//! shared crate (the derived list
//! of [`crate::checks::shared`]). The tool decides from the diff, never the
//! session, and the verdict records the mode and its reason.

use std::path::Path;

use oer_process::git;
use serde::{Deserialize, Serialize};

use crate::{Result, checks::shared::Owners};

/// The review a push gets.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The whole diff against the merge base.
    Full,
    /// The diff since the previous pushed head.
    Round,
}

/// The mode, why, and for a round the previous pushed head and the diff
/// from it to `HEAD`.
#[derive(Clone, Debug)]
pub struct Plan {
    pub mode: Mode,
    pub reason: String,
    pub pushed: Option<String>,
    pub diff: Option<String>,
}

impl Plan {
    fn full(reason: impl Into<String>, pushed: Option<&str>) -> Self {
        Self {
            mode: Mode::Full,
            reason: reason.into(),
            pushed: pushed.map(str::to_owned),
            diff: None,
        }
    }
}

/// The review of `head` (merge base `base`) for a branch whose previous
/// pushed head is `pushed`, `None` for its first push; `shared` names the
/// shared crate that owns a file.
pub fn plan(
    root: &Path,
    base: &str,
    head: &str,
    pushed: Option<&str>,
    shared: &Owners,
) -> Result<Plan> {
    let Some(pushed) = pushed else {
        return Ok(Plan::full("the first push of the branch", None));
    };
    if pushed == head {
        return Ok(Plan::full(
            "HEAD is the pushed head: nothing new to review in a round",
            Some(pushed),
        ));
    }
    if git::output(root, ["cat-file", "-e", &format!("{pushed}^{{commit}}")]).is_err() {
        return Ok(Plan::full(
            format!(
                "the previous pushed head {} is not in this checkout",
                short(pushed)
            ),
            Some(pushed),
        ));
    }
    let Some((from, diff)) = round_diff(root, base, head, pushed)? else {
        return Ok(Plan::full(
            "the previous pushed head does not rebase cleanly onto the merge base",
            Some(pushed),
        ));
    };
    let at = |revision: &str, file: &str| {
        git::output(root, ["show", &format!("{revision}:{file}")])
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    };
    let api_changed = |file: &str| {
        public_items(at(&from, file).as_deref()) != public_items(at(head, file).as_deref())
    };
    if let Some(reason) = widening(&diff, shared, api_changed) {
        return Ok(Plan::full(reason, Some(pushed)));
    }
    Ok(Plan {
        mode: Mode::Round,
        reason: format!("a fix round since the pushed head {}", short(pushed)),
        pushed: Some(pushed.to_owned()),
        diff: Some(diff),
    })
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// The revision the round starts from and the diff from it to `head`: the
/// previous pushed head when `head` descends from it, otherwise that head's
/// change replayed onto `base` (`git merge-tree`), so a rebase brings none
/// of `main`'s commits into the round; `None` when the replay conflicts.
fn round_diff(
    root: &Path,
    base: &str,
    head: &str,
    pushed: &str,
) -> Result<Option<(String, String)>> {
    let from = if git::output(root, ["merge-base", "--is-ancestor", pushed, head]).is_ok() {
        pushed.to_owned()
    } else {
        let pushed_base = git::text(root, ["merge-base", pushed, base])?;
        match git::text(
            root,
            [
                "merge-tree",
                "--write-tree",
                "--merge-base",
                &pushed_base,
                base,
                pushed,
            ],
        ) {
            // The first line names the merged tree.
            Ok(output) => output.lines().next().unwrap_or_default().to_owned(),
            Err(_) => return Ok(None),
        }
    };
    let diff = git::output(root, ["diff", "--no-ext-diff", "--no-color", &from, head])?;
    Ok(Some((from, String::from_utf8_lossy(&diff).into_owned())))
}

/// Words on a changed Rust line that name concurrency: threads, tasks,
/// atomics, locks and the traits that let a value cross them.
const CONCURRENCY: &[&str] = &[
    "Atomic",
    "Ordering::",
    "Mutex",
    "RwLock",
    "Condvar",
    "Barrier",
    "thread::",
    "spawn",
    "async ",
    ".await",
    "Send",
    "Sync",
    "critical_section",
    "FileLock",
    "flock",
];

/// Why the round's `diff` needs the full review, or `None`; `api_changed`
/// says whether a Rust file's public items differ across the round
/// ([`public_items`]).
pub fn widening(diff: &str, shared: &Owners, api_changed: impl Fn(&str) -> bool) -> Option<String> {
    let mut file = "";
    for line in diff.lines() {
        if let Some(paths) = line.strip_prefix("diff --git ") {
            file = paths.rsplit_once(" b/").map_or(paths, |(_, new)| new);
            if let Some(name) = shared.shared(file) {
                return Some(format!(
                    "the round changes the shared crate {name} ({file})"
                ));
            }
            if file.ends_with(".rs") && api_changed(file) {
                return Some(format!("the round changes the public API of {file}"));
            }
            continue;
        }
        if !file.ends_with(".rs") || line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        let Some(code) = line.strip_prefix('+').or_else(|| line.strip_prefix('-')) else {
            continue;
        };
        let code = code.trim_start();
        if code.starts_with("//") {
            continue;
        }
        if code.contains("unsafe") {
            return Some(format!("the round touches `unsafe` in {file}"));
        }
        if let Some(word) = CONCURRENCY.iter().find(|word| code.contains(*word)) {
            return Some(format!(
                "the round touches concurrency (`{}`) in {file}",
                word.trim()
            ));
        }
    }
    None
}

/// The public items of a Rust file, each as its tokens without function
/// bodies or doc comments: what a public API's shape is. An absent file has
/// none; one that does not parse yields an item of its own text, so any
/// change to it counts.
pub fn public_items(text: Option<&str>) -> Option<std::collections::BTreeSet<String>> {
    let Some(text) = text else {
        return Some(std::collections::BTreeSet::new());
    };
    let Ok(file) = syn::parse_file(text) else {
        return Some(std::collections::BTreeSet::from([text.to_owned()]));
    };
    let mut items = std::collections::BTreeSet::new();
    collect(&file.items, "", &mut items);
    Some(items)
}

fn collect(items: &[syn::Item], path: &str, out: &mut std::collections::BTreeSet<String>) {
    use quote::ToTokens;
    let public = |visibility: &syn::Visibility| matches!(visibility, syn::Visibility::Public(_));
    let undocumented = |attributes: &mut Vec<syn::Attribute>| {
        attributes.retain(|attribute| !attribute.path().is_ident("doc"));
    };
    for item in items {
        let mut item = item.clone();
        let text = match &mut item {
            syn::Item::Fn(function) if public(&function.vis) => {
                undocumented(&mut function.attrs);
                Some(format!(
                    "{}{}",
                    function
                        .attrs
                        .iter()
                        .map(|a| a.to_token_stream().to_string())
                        .collect::<String>(),
                    function.sig.to_token_stream()
                ))
            }
            syn::Item::Mod(module) if public(&module.vis) => {
                if let Some((_, inner)) = &module.content {
                    collect(inner, &format!("{path}{}::", module.ident), out);
                }
                Some(format!("mod {}", module.ident))
            }
            syn::Item::Trait(definition) if public(&definition.vis) => {
                undocumented(&mut definition.attrs);
                for member in &mut definition.items {
                    match member {
                        syn::TraitItem::Fn(function) => {
                            function.default = None;
                            undocumented(&mut function.attrs);
                        }
                        syn::TraitItem::Const(constant) => undocumented(&mut constant.attrs),
                        syn::TraitItem::Type(alias) => undocumented(&mut alias.attrs),
                        _ => {}
                    }
                }
                Some(definition.to_token_stream().to_string())
            }
            syn::Item::Impl(block) => {
                let header = format!(
                    "impl {} {} {} for {}",
                    block.generics.to_token_stream(),
                    block.generics.where_clause.to_token_stream(),
                    block
                        .trait_
                        .as_ref()
                        .map(|(_, name, _)| name.to_token_stream().to_string())
                        .unwrap_or_default(),
                    block.self_ty.to_token_stream()
                );
                // A trait impl is public API whatever its items' visibility.
                if block.trait_.is_some() {
                    out.insert(format!("{path}{header}"));
                }
                for member in &block.items {
                    match member {
                        syn::ImplItem::Fn(function)
                            if block.trait_.is_none() && public(&function.vis) =>
                        {
                            out.insert(format!(
                                "{path}{header} {}",
                                function.sig.to_token_stream()
                            ));
                        }
                        syn::ImplItem::Const(constant)
                            if block.trait_.is_some() || public(&constant.vis) =>
                        {
                            out.insert(format!("{path}{header} {}", constant.to_token_stream()));
                        }
                        // A trait impl's associated type is part of its API.
                        syn::ImplItem::Type(alias) if block.trait_.is_some() => {
                            out.insert(format!("{path}{header} {}", alias.to_token_stream()));
                        }
                        _ => {}
                    }
                }
                None
            }
            syn::Item::Struct(definition) if public(&definition.vis) => {
                undocumented(&mut definition.attrs);
                for field in &mut definition.fields {
                    undocumented(&mut field.attrs);
                }
                Some(definition.to_token_stream().to_string())
            }
            syn::Item::Enum(definition) if public(&definition.vis) => {
                undocumented(&mut definition.attrs);
                for variant in &mut definition.variants {
                    undocumented(&mut variant.attrs);
                    for field in &mut variant.fields {
                        undocumented(&mut field.attrs);
                    }
                }
                Some(definition.to_token_stream().to_string())
            }
            syn::Item::Type(alias) if public(&alias.vis) => {
                undocumented(&mut alias.attrs);
                Some(alias.to_token_stream().to_string())
            }
            syn::Item::Const(constant) if public(&constant.vis) => {
                undocumented(&mut constant.attrs);
                Some(constant.to_token_stream().to_string())
            }
            syn::Item::Static(value) if public(&value.vis) => {
                undocumented(&mut value.attrs);
                Some(value.to_token_stream().to_string())
            }
            syn::Item::Use(import) if public(&import.vis) => {
                Some(import.to_token_stream().to_string())
            }
            // An exported `macro_rules!` is public whatever its visibility.
            syn::Item::Macro(definition)
                if definition
                    .attrs
                    .iter()
                    .any(|attribute| attribute.path().is_ident("macro_export")) =>
            {
                undocumented(&mut definition.attrs);
                Some(definition.to_token_stream().to_string())
            }
            syn::Item::Union(definition) if public(&definition.vis) => {
                undocumented(&mut definition.attrs);
                Some(definition.to_token_stream().to_string())
            }
            _ => None,
        };
        if let Some(text) = text {
            out.insert(format!("{path}{text}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(file: &str, lines: &[&str]) -> String {
        format!(
            "diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n@@ -1 +1 @@\n{}\n",
            lines.join("\n")
        )
    }

    #[test]
    fn a_plain_fix_stays_a_round() {
        let round = diff(
            "crates/a/src/lib.rs",
            &["-    let x = 1;", "+    let x = 2;"],
        );
        assert_eq!(widening(&round, &Owners::default(), |_| false), None);
        // Comments and non-Rust files never widen it.
        let comment = diff("crates/a/src/lib.rs", &["+    // spawn a thread later"]);
        assert_eq!(widening(&comment, &Owners::default(), |_| false), None);
        let docs = diff("docs/a.md", &["+pub fn in prose, unsafe too"]);
        assert_eq!(widening(&docs, &Owners::default(), |_| true), None);
    }

    /// The public API is what the parsed public items say, wherever rustfmt
    /// puts their lines: a body, a private item or a doc comment is none of
    /// it.
    #[test]
    fn a_public_items_shape_is_its_api_and_a_body_is_not() {
        let api = |text: &str| public_items(Some(text)).unwrap();
        let base = api(
            "/// Opens.\npub fn open(\n    path: &Path,\n) -> File {\n    let x = 1;\n    todo!()\n}\n",
        );
        // A body, a doc comment or a private item changes nothing.
        assert_eq!(
            api(
                "/// Opens a file.\npub fn open(\n    path: &Path,\n) -> File {\n    let x = 2;\n    todo!()\n}\nfn helper() {}\n"
            ),
            base
        );
        // A parameter, a return type or a bound does.
        for changed in [
            "pub fn open(\n    path: &Path,\n    mode: Mode,\n) -> File {\n    todo!()\n}\n",
            "pub fn open(\n    path: &Path,\n) -> Result<File> {\n    todo!()\n}\n",
            "pub fn open<P>(\n    path: P,\n) -> File\nwhere\n    P: AsRef<Path>,\n{\n    todo!()\n}\n",
        ] {
            assert_ne!(api(changed), base, "{changed}");
        }
        // A variant's field, a trait method after a default one, a
        // supertrait, a public type's bound.
        let pairs = [
            (
                "pub enum E {\n    A { x: u8 },\n}\n",
                "pub enum E {\n    A { x: u16 },\n}\n",
            ),
            (
                "pub trait T {\n    fn a(&self) {\n        let x = 1;\n    }\n    fn b(&self);\n}\n",
                "pub trait T {\n    fn a(&self) {\n        let x = 1;\n    }\n    fn b(&self, y: u8);\n}\n",
            ),
            ("pub trait T: Send {}\n", "pub trait T: Send + Sync {}\n"),
            (
                "pub struct Q<T>\nwhere\n    T: Copy,\n{\n    x: T,\n}\n",
                "pub struct Q<T>\nwhere\n    T: Clone,\n{\n    x: T,\n}\n",
            ),
            ("pub type A = u8;\n", "pub type A = u16;\n"),
            (
                "pub struct S;\nimpl S {\n    pub fn new() -> Self { S }\n}\n",
                "pub struct S;\nimpl S {\n    pub fn new(x: u8) -> Self { S }\n}\n",
            ),
        ];
        for (old, new) in pairs {
            assert_ne!(api(old), api(new), "{new}");
        }
        // A default method's body is no API.
        assert_eq!(
            api("pub trait T {\n    fn a(&self) {\n        let x = 1;\n    }\n}\n"),
            api("pub trait T {\n    fn a(&self) {\n        let x = 2;\n    }\n}\n")
        );
        // An impl's bounds and a trait impl's associated type.
        assert_ne!(
            api("impl<T> Port for Q<T> where T: Copy {}\n"),
            api("impl<T> Port for Q<T> where T: Clone {}\n")
        );
        assert_ne!(
            api("impl Iterator for Q {\n    type Item = u8;\n}\n"),
            api("impl Iterator for Q {\n    type Item = u16;\n}\n")
        );
        // An exported macro is API, a private one is not.
        assert_ne!(
            api("#[macro_export]\nmacro_rules! m { () => { 1 }; }\n"),
            api("#[macro_export]\nmacro_rules! m { () => { 2 }; }\n")
        );
        assert_eq!(
            api("macro_rules! m { () => { 1 }; }\n"),
            api("macro_rules! m { () => { 2 }; }\n")
        );
        // A new or removed file without public items is no API.
        assert_eq!(public_items(None), public_items(Some("fn helper() {}\n")));
        // A file that does not parse counts any change.
        assert_ne!(api("fn ("), api("fn (("));
    }

    #[test]
    fn unsafe_concurrency_a_public_api_or_a_shared_crate_widen_a_round() {
        let reason = |lines: &[&str]| {
            widening(
                &diff("crates/a/src/lib.rs", lines),
                &Owners::default(),
                |_| false,
            )
            .unwrap()
        };
        assert!(reason(&["+    unsafe { read() }"]).contains("`unsafe`"));
        assert!(
            reason(&[
                "-    let held = lock.lock();",
                "+    let held = Mutex::new(0);"
            ])
            .contains("concurrency")
        );
        assert!(
            reason(&["+    counter.fetch_add(1, Ordering::Relaxed);"]).contains("`Ordering::`")
        );
        // A file whose public items changed.
        let api = widening(
            &diff("crates/a/src/lib.rs", &["+    let x = 2;"]),
            &Owners::default(),
            |file| file == "crates/a/src/lib.rs",
        );
        assert!(api.unwrap().contains("public API of crates/a/src/lib.rs"));
        let shared = Owners::new(
            [
                ("crates/time".to_owned(), "oer-time".to_owned()),
                (
                    "crates/time-embassy".to_owned(),
                    "oer-time-embassy".to_owned(),
                ),
            ],
            &["oer-time".to_owned()].into(),
        );
        let reason = widening(
            &diff("crates/time/src/lib.rs", &["+    let x = 2;"]),
            &shared,
            |_| false,
        )
        .unwrap();
        assert!(reason.contains("shared crate oer-time"), "{reason}");
        // A path that only starts like the crate's is another crate.
        assert_eq!(
            widening(
                &diff("crates/time-embassy/src/lib.rs", &["+    let x = 2;"]),
                &shared,
                |_| false
            ),
            None
        );
    }

    fn repository() -> (tempfile::TempDir, impl Fn(&[&str]) -> String) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let git = move |arguments: &[&str]| {
            git::text(
                &root,
                [
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "-c",
                        "commit.gpgsign=false",
                    ],
                    arguments,
                ]
                .concat(),
            )
            .unwrap()
        };
        git(&["init", "--quiet", "--initial-branch=main"]);
        (directory, git)
    }

    #[test]
    fn a_first_push_is_full_and_a_later_one_a_round_of_its_own_change() {
        let (directory, git) = repository();
        let root = directory.path();
        let commit = |name: &str, text: &str| {
            std::fs::write(root.join(name), text).unwrap();
            git(&["add", name]);
            git(&["commit", "--quiet", "-m", name]);
            git(&["rev-parse", "HEAD"])
        };
        let base = commit("main.txt", "main\n");
        git(&["checkout", "--quiet", "-b", "fix"]);
        let pushed = commit("a.rs", "fn a() {}\n");
        let first = plan(root, &base, &pushed, None, &Owners::default()).unwrap();
        assert_eq!(first.mode, Mode::Full);
        assert_eq!(first.reason, "the first push of the branch");
        let fixed = commit("a.rs", "fn a() { let x = 1; }\n");
        let round = plan(root, &base, &fixed, Some(&pushed), &Owners::default()).unwrap();
        assert_eq!(round.mode, Mode::Round, "{}", round.reason);
        let text = round.diff.unwrap();
        assert!(
            text.contains("+fn a() { let x = 1; }") && !text.contains("main.txt"),
            "{text}"
        );
        // After a rebase onto a newer main, main's commits stay out of the
        // round.
        git(&["checkout", "--quiet", "main"]);
        let newer = commit("other.txt", "newer main\n");
        git(&["checkout", "--quiet", "fix"]);
        git(&["rebase", "--quiet", "main"]);
        let rebased = git(&["rev-parse", "HEAD"]);
        let round = plan(root, &newer, &rebased, Some(&fixed), &Owners::default()).unwrap();
        assert_eq!(round.mode, Mode::Round, "{}", round.reason);
        let text = round.diff.unwrap();
        assert!(
            !text.contains("other.txt") && text.trim().is_empty(),
            "{text}"
        );
        // A pushed head this checkout lacks makes the review full.
        let missing = plan(
            root,
            &newer,
            &rebased,
            Some("0123456789abcdef0123456789abcdef01234567"),
            &Owners::default(),
        )
        .unwrap();
        assert_eq!(missing.mode, Mode::Full);
        assert!(
            missing.reason.contains("is not in this checkout"),
            "{}",
            missing.reason
        );
    }
}
