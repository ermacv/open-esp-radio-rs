//! Push this checkout's commits to `main` only after they pass
//! `check changed` on top of the `main` they land on.
//!
//! Hand-written `cargo xtask check changed | tail && git push` chains push
//! even when the check fails, because the pipeline's status is `tail`'s, and
//! they check a tree that `main` may already have moved past. This command
//! rebases onto `origin/main`, runs the check in a fresh `cargo xtask` (the
//! rebase may have changed xtask itself), rebases and checks again while
//! `main` keeps moving, and pushes only a revision that passed. When the
//! pushed commits change the stand's own tooling it reinstalls `oer-stand`.
use std::process::Command;

use crate::{Context, Result, process};

/// Rebases after which a push gives up rather than chasing a busy `main`.
const ATTEMPTS: usize = 3;

/// Paths whose change makes the installed `oer-stand` stale.
const STAND_TOOLING: &[&str] = &["tools/xtask/", "tools/process/", "hil/host/", "hil/schema/"];

fn git(ctx: &Context) -> Command {
    ctx.command("git")
}

fn text(command: &mut Command) -> Result<String> {
    Ok(String::from_utf8(process::capture(command)?.stdout)?
        .trim()
        .to_owned())
}

/// Tracked files with uncommitted changes: the check would see them, the
/// push would not carry them.
fn uncommitted(ctx: &Context) -> Result<Vec<String>> {
    Ok(
        text(git(ctx).args(["status", "--porcelain", "--untracked-files=no"]))?
            .lines()
            .map(|line| line.get(3..).unwrap_or(line).to_owned())
            .collect(),
    )
}

fn origin_main(ctx: &Context) -> Result<String> {
    process::capture(git(ctx).args(["fetch", "--quiet", "origin", "main"]))?;
    text(git(ctx).args(["rev-parse", "origin/main"]))
}

/// Whether the pushed range changes the stand's own tooling.
pub fn touches_stand_tooling(paths: &[&str]) -> bool {
    paths
        .iter()
        .any(|path| STAND_TOOLING.iter().any(|prefix| path.starts_with(prefix)))
}

/// Rebase, check, push; see the module documentation.
pub fn run(ctx: &Context) -> Result<()> {
    let dirty = uncommitted(ctx)?;
    if !dirty.is_empty() {
        return Err(format!(
            "push: uncommitted changes would be checked but not pushed; commit or stash them first: {}",
            dirty.join(", ")
        )
        .into());
    }
    let branch = text(git(ctx).args(["rev-parse", "--abbrev-ref", "HEAD"]))?;
    for attempt in 1..=ATTEMPTS {
        let base = origin_main(ctx)?;
        if let Err(error) = process::capture(git(ctx).args(["rebase", "--quiet", "origin/main"])) {
            let _ = process::capture(git(ctx).args(["rebase", "--abort"]));
            return Err(format!(
                "push: rebase onto origin/main failed; resolve it by hand: {error}"
            )
            .into());
        }
        let head = text(git(ctx).args(["rev-parse", "HEAD"]))?;
        if head == base {
            println!("push: nothing to push; {branch} is origin/main");
            return Ok(());
        }
        println!(
            "push: checking {} on origin/main {}",
            &head[..12],
            &base[..12]
        );
        process::run(ctx.cargo().args(["xtask", "check", "changed"]))
            .map_err(|error| format!("push: check changed failed; nothing pushed: {error}"))?;
        if origin_main(ctx)? != base {
            println!(
                "push: origin/main moved during the check; rebasing again ({attempt}/{ATTEMPTS})"
            );
            continue;
        }
        match process::capture(git(ctx).args(["push", "--quiet", "origin", "HEAD:main"])) {
            Ok(_) => {}
            Err(error) if attempt < ATTEMPTS => {
                println!("push: rejected ({error}); rebasing again ({attempt}/{ATTEMPTS})");
                continue;
            }
            Err(error) => return Err(error),
        }
        println!("push: pushed {} to main", &head[..12]);
        let changed = text(git(ctx).args(["diff", "--name-only", &base, &head]))?;
        if touches_stand_tooling(&changed.lines().collect::<Vec<_>>()) {
            println!("push: the stand tooling changed; reinstalling oer-stand");
            process::run(ctx.cargo().args(["xtask", "stand-install"]))?;
        }
        return Ok(());
    }
    Err(format!("push: origin/main kept moving through {ATTEMPTS} checks; run it again").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_stand_tooling_reinstalls_the_stand() {
        assert!(touches_stand_tooling(&["tools/xtask/src/hil.rs"]));
        assert!(touches_stand_tooling(&[
            "docs/x.md",
            "hil/host/runner/src/main.rs"
        ]));
        assert!(!touches_stand_tooling(&[
            "crates/trace/src/lib.rs",
            "hil/scenarios/a.toml"
        ]));
    }
}
