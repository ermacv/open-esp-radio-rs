//! `cargo hil run ... --baseline REV`: run a clean revision beside the tree.
//!
//! The revision is checked out, detached, in this checkout's baseline
//! worktree `target/hil/baseline/checkout`, and the runner and firmware are
//! built there, so the working tree is never touched and the run is an honest
//! clean run of that commit: its runner and images speak the same protocol.
//! The worktree keeps its own `target/`, so later baselines build
//! incrementally. The stand itself is shared, so a revision whose runner
//! predates the stand's state schema is refused before anything is built.

use std::{ffi::OsString, fs::File, path::Path};

use crate::{Context, Result};

const OPTION: &str = "--baseline";
/// The worktree below the checkout's root.
const WORKTREE: &str = "target/hil/baseline/checkout";
/// The file declaring the arbiter's state schema.
const STATE_SOURCE: &str = "hil/host/arbiter/src/state.rs";

/// Split `--baseline REV` (or `--baseline=REV`) from runner arguments, up to
/// a `--`.
pub(crate) fn take(args: Vec<OsString>) -> Result<(Option<String>, Vec<OsString>)> {
    let mut baseline = None;
    let mut remaining = Vec::new();
    let mut rest = args.into_iter();
    while let Some(argument) = rest.next() {
        let text = argument.to_str().unwrap_or_default();
        let value = if text == "--" {
            remaining.push(argument);
            remaining.extend(rest.by_ref());
            break;
        } else if text == OPTION {
            rest.next()
                .and_then(|value| value.into_string().ok())
                .ok_or("--baseline requires a revision")?
        } else if let Some(value) = text.strip_prefix("--baseline=") {
            value.to_owned()
        } else {
            remaining.push(argument);
            continue;
        };
        if baseline.replace(value).is_some() {
            return Err("--baseline is given twice".into());
        }
    }
    Ok((baseline, remaining))
}

/// Check `revision` out in the baseline worktree and return the context of
/// that worktree, with the lock that keeps other baselines of this checkout
/// from switching it until the run ends.
pub(crate) fn checkout(ctx: &Context, revision: &str) -> Result<(Context, File)> {
    let commit = git_text(
        &ctx.root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .map_err(|_| format!("--baseline {revision} names no commit"))?;
    let current = state_schema(&std::fs::read_to_string(ctx.root.join(STATE_SOURCE))?)
        .ok_or("the checkout's stand state schema is unreadable")?;
    let baseline = git_text(&ctx.root, &["show", &format!("{commit}:{STATE_SOURCE}")])
        .ok()
        .and_then(|source| state_schema(&source));
    if baseline != Some(current) {
        return Err(format!(
            "--baseline {revision}: its runner uses stand state schema {}, the stand uses \
             {current}; choose a revision at or after the stand's last schema change",
            baseline.map_or_else(|| String::from("unknown"), |schema| schema.to_string())
        )
        .into());
    }
    let worktree = ctx.root.join(WORKTREE);
    let parent = worktree.parent().ok_or("baseline worktree has no parent")?;
    std::fs::create_dir_all(parent)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join("lock"))?;
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        eprintln!("hil: waiting for this checkout's other baseline run");
        fs2::FileExt::lock_exclusive(&lock)?;
    }
    if worktree.join(".git").exists() {
        git(
            &worktree,
            &["checkout", "--quiet", "--detach", "--force", &commit],
        )?;
        // Ignored build outputs stay; anything else left behind goes.
        git(&worktree, &["clean", "-fdq"])?;
    } else {
        git(&ctx.root, &["worktree", "prune"])?;
        git(
            &ctx.root,
            &[
                "worktree",
                "add",
                "--detach",
                "--force",
                &worktree.to_string_lossy(),
                &commit,
            ],
        )?;
    }
    eprintln!(
        "hil: baseline {revision} ({}) checked out in {}",
        &commit[..12.min(commit.len())],
        worktree.display()
    );
    Ok((Context::new(&worktree)?, lock))
}

/// `N` from the `STATE_SCHEMA: u32 = N;` declaration in the arbiter's state
/// source.
fn state_schema(source: &str) -> Option<u32> {
    source.lines().find_map(|line| {
        line.split_once("STATE_SCHEMA: u32 =")?
            .1
            .trim()
            .strip_suffix(';')?
            .parse()
            .ok()
    })
}

fn git(root: &Path, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()?;
    if !status.success() {
        return Err(format!("git {} failed in {}", args.join(" "), root.display()).into());
    }
    Ok(())
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!("git {} failed", args.join(" ")).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_baseline_option_is_taken_before_a_double_dash() {
        let (baseline, rest) = take(args(&[
            "run",
            "x",
            "--baseline",
            "main~3",
            "y",
            "--",
            "--baseline",
        ]))
        .unwrap();
        assert_eq!(baseline.as_deref(), Some("main~3"));
        assert_eq!(rest, args(&["run", "x", "y", "--", "--baseline"]));
        let (baseline, _) = take(args(&["run", "--baseline=abc"])).unwrap();
        assert_eq!(baseline.as_deref(), Some("abc"));
        assert!(take(args(&["run", "--baseline"])).is_err());
        assert!(take(args(&["run", "--baseline=a", "--baseline", "b"])).is_err());
        assert_eq!(take(args(&["run", "x"])).unwrap().0, None);
    }

    #[test]
    fn the_state_schema_is_read_from_its_declaration() {
        assert_eq!(
            state_schema("x\npub(crate) const STATE_SCHEMA: u32 = 3;\n"),
            Some(3)
        );
        assert_eq!(state_schema("const OTHER: u32 = 3;"), None);
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(STATE_SOURCE),
        )
        .unwrap();
        assert!(state_schema(&source).is_some());
    }
}
