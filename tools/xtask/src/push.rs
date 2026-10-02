//! Push this checkout's branch after the gate passed on exactly the tree that
//! is pushed, and hand it to CI through its pull request.
//!
//! 1. The tree must be `HEAD`: an uncommitted change to a tracked file, or
//!    an untracked Rust source or Cargo manifest or lock that a build would
//!    read, refuses the push, since the gate would check what is not pushed.
//! 2. The branch is not `main`: `main` changes only through pull requests,
//!    which CI checks and merges.
//! 3. The fast gate ([`crate::gate`]) checks the packages `HEAD`'s commits
//!    change against the merge base with `origin/main`.
//! 4. Push the branch, open its pull request when it has none, and enable
//!    auto-merge: GitHub rebases it onto `main` once CI passes. With
//!    `--draft` the pull request is a draft and nothing merges it.
//!
//! The push never waits for CI and never rebases: a branch that `main`
//! outran merges as it is, and CI on `main` checks the result. A branch the
//! developer rebased replaces its remote copy with `--force-with-lease`
//! bound to the copy this push saw ([`Update::Rewrite`]), so a commit pushed
//! there by anyone else in the meantime refuses the push instead of being
//! lost.

use oer_process as process;

use crate::{Context, Result, gate};

/// The base pull requests merge into.
const MAIN: &str = "main";

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

/// Why `branch` cannot be pushed for review, if it cannot.
pub fn unreviewable(branch: &str) -> Option<String> {
    match branch {
        "HEAD" => Some(String::from(
            "HEAD is detached; create a branch for the change first",
        )),
        MAIN => Some(String::from(
            "main changes only through pull requests; create a branch for the change first",
        )),
        _ => None,
    }
}

/// How a push updates the branch's remote copy.
#[derive(Debug, Eq, PartialEq)]
pub enum Update {
    /// The branch has no remote copy yet, or `HEAD` descends from it.
    FastForward,
    /// `HEAD` rewrote the branch, a rebase: replace the remote copy only
    /// while it is still `seen`.
    Rewrite { seen: String },
}

/// The update for a branch whose remote copy is `remote`, when `HEAD`
/// `descends` from it or not.
pub fn update(remote: Option<&str>, descends: bool) -> Update {
    match remote {
        Some(seen) if !descends => Update::Rewrite {
            seen: seen.to_owned(),
        },
        _ => Update::FastForward,
    }
}

fn git(ctx: &Context, arguments: &[&str]) -> Result<String> {
    Ok(gate::git(ctx, arguments)?.trim().to_owned())
}

fn gh(ctx: &Context, arguments: &[&str]) -> Result<String> {
    let output = process::capture(ctx.command("gh").args(arguments))?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Gate, push, open the pull request; see the module documentation.
pub fn run(ctx: &Context, draft: bool) -> Result<()> {
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
    let branch = git(ctx, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if let Some(reason) = unreviewable(&branch) {
        return Err(format!("push: {reason}").into());
    }
    process::capture(
        ctx.command("git")
            .args(["fetch", "--quiet", "origin", MAIN]),
    )?;
    let base = git(ctx, &["merge-base", "HEAD", &format!("origin/{MAIN}")])?;
    let head = git(ctx, &["rev-parse", "HEAD"])?;
    if head == base {
        println!("push: nothing to push; HEAD is in origin/{MAIN}");
        return Ok(());
    }
    let files = gate::committed(ctx, &base)?;
    let tree = gate::Tree::load(&ctx.root)?;
    let mut selection = gate::select(&tree, &files);
    gate::select_locks(ctx, &tree, &files, &base, None, &mut selection)?;
    let affected = gate::affected(ctx, &selection)?;
    println!(
        "push: gating {} on {}: {} files, {} package(s) with dependents",
        &head[..12],
        &base[..12],
        files.len(),
        affected.len()
    );
    gate::run(ctx, &tree, &selection, &affected, gate::Depth::Fast)
        .map_err(|error| format!("push: the gate failed; nothing pushed: {error}"))?;
    let reference = format!("refs/heads/{branch}");
    let remote = git(ctx, &["ls-remote", "origin", &reference])?
        .split_whitespace()
        .next()
        .map(str::to_owned);
    let descends = match &remote {
        Some(seen) => {
            process::capture(
                ctx.command("git")
                    .args(["fetch", "--quiet", "origin", &reference]),
            )?;
            process::capture(
                ctx.command("git")
                    .args(["merge-base", "--is-ancestor", seen, "HEAD"]),
            )
            .is_ok()
        }
        None => true,
    };
    let update = update(remote.as_deref(), descends);
    let mut command = ctx.command("git");
    command.args(["push", "--quiet", "--set-upstream"]);
    if let Update::Rewrite { seen } = &update {
        command.arg(format!("--force-with-lease={reference}:{seen}"));
    }
    process::capture(command.args(["origin", &format!("HEAD:{reference}")]))
        .map_err(|error| format!("push: git push of {branch} failed: {error}"))?;
    match &update {
        Update::FastForward => println!("push: pushed {} to {branch}", &head[..12]),
        Update::Rewrite { seen } => println!(
            "push: rewrote {branch} from {} to {}",
            &seen[..seen.len().min(12)],
            &head[..12]
        ),
    }
    let existing = gh(
        ctx,
        &["pr", "view", &branch, "--json", "url", "--jq", ".url"],
    )
    .ok()
    .filter(|url| !url.is_empty());
    let opened = existing.is_none();
    let url = match existing {
        Some(url) => url,
        None => {
            let mut create = vec![
                "pr",
                "create",
                "--fill-first",
                "--base",
                MAIN,
                "--head",
                &branch,
            ];
            if draft {
                create.push("--draft");
            }
            gh(ctx, &create).map_err(|error| format!("push: gh pr create failed: {error}"))?
        }
    };
    if draft {
        if opened {
            println!("push: opened draft pull request {url}");
        } else {
            println!("push: updated pull request {url}; auto-merge left as it was");
        }
    } else {
        gh(ctx, &["pr", "merge", &branch, "--auto", "--rebase"])
            .map_err(|error| format!("push: enabling auto-merge of {url} failed: {error}"))?;
        println!("push: {url} merges into {MAIN} once CI passes");
    }
    crate::ci_status::print(ctx, "push");
    Ok(())
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
    fn only_a_rebased_branch_is_rewritten_and_only_from_what_was_seen() {
        assert_eq!(update(None, true), Update::FastForward);
        assert_eq!(update(Some("abc"), true), Update::FastForward);
        assert_eq!(
            update(Some("abc"), false),
            Update::Rewrite {
                seen: String::from("abc")
            }
        );
    }

    #[test]
    fn main_changes_only_through_pull_requests() {
        assert!(unreviewable("main").unwrap().contains("pull requests"));
        assert!(unreviewable("HEAD").unwrap().contains("detached"));
        assert_eq!(unreviewable("fix-publication"), None);
    }
}
