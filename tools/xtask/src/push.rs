//! Push this checkout's commits to `main` only after the gate passed on
//! exactly the tree that is pushed.
//!
//! 1. The tree must be `HEAD`: an uncommitted change to a tracked file, or
//!    an untracked Rust source or Cargo manifest or lock that a build would
//!    read, refuses the push, since the gate would check what is not pushed.
//! 2. The gate ([`crate::gate`]) checks the packages `HEAD`'s commits
//!    change, and their dependents, against the merge base with
//!    `origin/main`.
//! 3. Fetch and rebase onto `origin/main`. When the incoming commits affect
//!    no package the pushed commits affect, push at once; otherwise rerun
//!    the gate for the packages both affect. A push that `main` outran in
//!    the meantime is rejected as non-fast-forward and goes round again.
//!
//! No lock, queue or nested `cargo xtask` is involved: Git's
//! fast-forward check is the only serialization `main` needs.

use std::collections::BTreeSet;

use oer_process as process;

use crate::{Context, Result, gate};

/// Rounds before giving up while `main` keeps moving under the push.
const ROUNDS: usize = 5;

/// The base the push lands on.
const MAIN: &str = "origin/main";

/// Files whose untracked presence changes what a build of the tree reads.
pub fn build_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.ends_with(".rs") || name == "Cargo.toml" || name == "Cargo.lock"
}

/// Why the working tree is not `HEAD`, if it is not.
pub fn unpushable(tracked: &[String], untracked: &[String]) -> Option<String> {
    if !tracked.is_empty() {
        return Some(format!(
            "uncommitted changes would be checked but not pushed; commit or stash them: {}",
            tracked.join(", ")
        ));
    }
    let inputs: Vec<&str> = untracked
        .iter()
        .map(String::as_str)
        .filter(|path| build_input(path))
        .collect();
    (!inputs.is_empty()).then(|| {
        format!(
            "untracked build inputs would be checked but not pushed; add, ignore or remove them: {}",
            inputs.join(", ")
        )
    })
}

/// The packages two changes both affect.
pub fn overlap(ours: &BTreeSet<gate::Key>, theirs: &BTreeSet<gate::Key>) -> BTreeSet<gate::Key> {
    ours.intersection(theirs).cloned().collect()
}

fn git(ctx: &Context, arguments: &[&str]) -> Result<String> {
    Ok(gate::git(ctx, arguments)?.trim().to_owned())
}

fn fetch(ctx: &Context) -> Result<String> {
    process::capture(
        ctx.command("git")
            .args(["fetch", "--quiet", "origin", "main"]),
    )?;
    git(ctx, &["rev-parse", MAIN])
}

fn changed(ctx: &Context, from: &str, to: &str) -> Result<Vec<String>> {
    Ok(git(ctx, &["diff", "--name-only", from, to])?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Gate, rebase, push; see the module documentation.
pub fn run(ctx: &Context) -> Result<()> {
    let tracked: Vec<String> = git(ctx, &["status", "--porcelain", "--untracked-files=no"])?
        .lines()
        .map(|line| line.get(3..).unwrap_or(line).to_owned())
        .collect();
    let untracked: Vec<String> = git(ctx, &["ls-files", "--others", "--exclude-standard"])?
        .lines()
        .map(str::to_owned)
        .collect();
    if let Some(reason) = unpushable(&tracked, &untracked) {
        return Err(format!("push: {reason}").into());
    }
    let mut main = fetch(ctx)?;
    let mut base = git(ctx, &["merge-base", "HEAD", &main])?;
    let head = git(ctx, &["rev-parse", "HEAD"])?;
    if head == base {
        println!("push: nothing to push; HEAD is in {MAIN}");
        return Ok(());
    }
    let ours = changed(ctx, &base, "HEAD")?;
    let tree = gate::Tree::load(&ctx.root)?;
    let selection = gate::select(&tree, &ours);
    let affected = gate::affected(ctx, &selection)?;
    println!(
        "push: gating {} on {}: {} files, {} package(s) with dependents",
        &head[..12],
        &base[..12],
        ours.len(),
        affected.len()
    );
    gate::run(ctx, &tree, &selection, &affected)
        .map_err(|error| format!("push: the gate failed; nothing pushed: {error}"))?;
    for round in 1..=ROUNDS {
        if base != main {
            let theirs = changed(ctx, &base, &main)?;
            if let Err(error) =
                process::capture(ctx.command("git").args(["rebase", "--quiet", MAIN]))
            {
                let _ = process::capture(ctx.command("git").args(["rebase", "--abort"]));
                return Err(format!(
                    "push: rebase onto {MAIN} failed; resolve it by hand: {error}"
                )
                .into());
            }
            let rebased = gate::Tree::load(&ctx.root)?;
            let incoming = gate::affected(ctx, &gate::select(&rebased, &theirs))?;
            let both = overlap(&affected, &incoming);
            if both.is_empty() {
                println!(
                    "push: {MAIN} moved to {}; its {} changed files affect none of these packages",
                    &main[..12],
                    theirs.len()
                );
            } else {
                println!(
                    "push: {MAIN} moved to {}; regating the {} package(s) both changes affect",
                    &main[..12],
                    both.len()
                );
                let mut files = ours.clone();
                files.extend(theirs);
                let mut selection = gate::select(&rebased, &files);
                selection.packages.retain(|key| both.contains(key));
                gate::run(ctx, &rebased, &selection, &both).map_err(|error| {
                    format!("push: the gate failed on {MAIN}; nothing pushed: {error}")
                })?;
            }
            base = main.clone();
        }
        let head = git(ctx, &["rev-parse", "HEAD"])?;
        match process::capture(
            ctx.command("git")
                .args(["push", "--quiet", "origin", "HEAD:main"]),
        ) {
            Ok(_) => {
                println!("push: pushed {} to main", &head[..12]);
                crate::ci_status::print(ctx, "push");
                return Ok(());
            }
            Err(error) => {
                println!(
                    "push: rejected ({round}/{ROUNDS}): {}",
                    first_line(&error.to_string())
                );
            }
        }
        main = fetch(ctx)?;
    }
    Err(format!("push: {MAIN} moved {ROUNDS} times during this push; run it again").into())
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| line.contains("rejected") || line.contains("error"))
        .unwrap_or_else(|| text.lines().next().unwrap_or(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn only_the_committed_tree_is_pushed() {
        assert_eq!(unpushable(&[], &strings(&["notes.txt", "docs/x.md"])), None);
        let reason = unpushable(&strings(&["crates/a/src/lib.rs"]), &[]).unwrap();
        assert!(reason.contains("uncommitted"), "{reason}");
        let reason = unpushable(
            &[],
            &strings(&["crates/a/src/new.rs", "notes.txt", "crates/b/Cargo.toml"]),
        )
        .unwrap();
        assert!(
            reason.contains("crates/a/src/new.rs, crates/b/Cargo.toml"),
            "{reason}"
        );
        assert!(!reason.contains("notes.txt"), "{reason}");
        assert!(build_input("Cargo.lock") && !build_input("README.md"));
    }

    #[test]
    fn disjoint_changes_need_no_second_gate() {
        let key = |name: &str| (String::from("Cargo.toml"), name.to_owned());
        let ours = BTreeSet::from([key("a"), key("b")]);
        assert!(overlap(&ours, &BTreeSet::from([key("c")])).is_empty());
        assert_eq!(
            overlap(&ours, &BTreeSet::from([key("b"), key("c")])),
            BTreeSet::from([key("b")])
        );
    }
}
