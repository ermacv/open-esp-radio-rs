//! Verification of `main` after a push, off every agent's critical path.
//!
//! `cargo xtask push` checks a change with the cheap part of `check changed`
//! (formatting, lints, tests of the changed packages and a type check of the
//! firmware classes it reaches) and pushes. The expensive part, building and
//! auditing the reached image classes, runs afterwards in the background: a
//! push starts `cargo xtask verify-main`, which builds the newest `main` in a
//! dedicated worktree of the host and records the outcome. One verifier runs
//! at a time; a push while it runs is covered by its next round, which
//! verifies whatever `main` is then.
//!
//! The outcome is information, not a gate: `check changed` and `push` print
//! a failed verification with the range of commits since the last verified
//! `main`, and whoever pushed into that range fixes it first. An image that
//! reaches a board is still built and audited by the run that flashes it.

use crate::{Context, Result, process};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// What the last verification found.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Status {
    /// The newest `main` whose reached classes all built and passed their
    /// audits.
    pub verified: Option<String>,
    /// A `main` whose verification failed, while no later one passed.
    pub failed: Option<Failure>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Failure {
    /// The `main` that failed.
    pub head: String,
    /// The last verified `main` before it: the culprit is in `base..head`.
    pub base: Option<String>,
    /// The verifier's output.
    pub log: PathBuf,
}

impl Status {
    /// The status after verifying `head` from `base` with `passed`.
    fn after(self, base: Option<String>, head: &str, passed: bool, log: PathBuf) -> Self {
        if passed {
            Self {
                verified: Some(head.to_owned()),
                failed: None,
            }
        } else {
            Self {
                verified: self.verified,
                // The first failure names the smallest range; keep it until
                // a later `main` passes.
                failed: self.failed.or(Some(Failure {
                    head: head.to_owned(),
                    base,
                    log,
                })),
            }
        }
    }

    /// A line for `check changed` and `push`, when verification failed.
    pub fn report(&self) -> Option<String> {
        let failure = self.failed.as_ref()?;
        let range = match &failure.base {
            Some(base) => format!("{}..{}", short(base), short(&failure.head)),
            None => short(&failure.head).to_owned(),
        };
        Some(format!(
            "main verification failed for {range}: an image class of main no longer builds or passes its audits; the culprit is in that range; see {}",
            failure.log.display()
        ))
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// The verifier's directory in the host's cache.
fn directory() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or("HOME is required to locate the main verifier")?;
    Ok(base.join("open-esp-radio/verify-main"))
}

/// The recorded status; empty before the first verification.
pub fn status() -> Result<Status> {
    match fs::read(directory()?.join("status.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Status::default()),
        Err(error) => Err(error.into()),
    }
}

fn save(status: &Status) -> Result<()> {
    let directory = directory()?;
    fs::create_dir_all(&directory)?;
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer_pretty(&mut file, status)?;
    file.persist(directory.join("status.json"))?;
    Ok(())
}

/// Start `cargo xtask verify-main` detached from the pushing session.
pub fn in_background(root: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    Command::new(std::env::current_exe()?)
        .arg("--root")
        .arg(root)
        .arg("verify-main")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
}

fn git(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory);
    command
}

fn text(command: &mut Command) -> Result<String> {
    Ok(String::from_utf8(process::capture(command)?.stdout)?
        .trim()
        .to_owned())
}

/// Verify `main` until the verified head is the newest; a verifier that
/// finds another one running leaves the work to it.
pub fn run(ctx: &Context) -> Result<()> {
    let directory = directory()?;
    fs::create_dir_all(&directory)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory.join("lock"))?;
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        return Ok(());
    }
    let checkout = directory.join("checkout");
    loop {
        process::capture(git(&ctx.root).args(["fetch", "--quiet", "origin", "main"]))?;
        let head = text(git(&ctx.root).args(["rev-parse", "origin/main"]))?;
        let before = status()?;
        let last = before
            .failed
            .as_ref()
            .map(|failure| failure.head.clone())
            .or_else(|| before.verified.clone());
        if last.as_deref() == Some(head.as_str()) {
            return Ok(());
        }
        if !checkout.join(".git").exists() {
            process::capture(
                git(&ctx.root)
                    .args(["worktree", "add", "--detach"])
                    .arg(&checkout)
                    .arg(&head),
            )?;
        } else {
            process::capture(git(&checkout).args(["checkout", "--quiet", "--detach", &head]))?;
        }
        let base = before.verified.clone();
        let changed = match &base {
            Some(base) => text(git(&checkout).args(["diff", "--name-only", base, &head]))?
                .lines()
                .map(PathBuf::from)
                .collect(),
            None => Vec::new(),
        };
        let classes = if base.is_some() {
            crate::checks::firmware::affected(&changed)?
        } else {
            oer_hil_runner_core::image::ImageClass::ALL.to_vec()
        };
        let log = directory.join(format!("{}.log", short(&head)));
        let passed = classes.is_empty() || {
            let output = fs::File::create(&log)?;
            let mut check = Command::new(std::env::current_exe()?);
            check
                .arg("--root")
                .arg(&checkout)
                .args(["check", "firmware"]);
            for class in &classes {
                check.args(["--class", class.id()]);
            }
            check
                .stdout(output.try_clone()?)
                .stderr(output)
                .status()?
                .success()
        };
        save(&before.after(base, &head, passed, log))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_failure_is_kept_until_a_later_main_passes() {
        let log = PathBuf::from("/v/b.log");
        let status = Status::default().after(None, "a", true, PathBuf::from("/v/a.log"));
        assert_eq!(status.verified.as_deref(), Some("a"));
        assert!(status.report().is_none());

        let failed = status.after(Some("a".into()), "b", false, log.clone());
        let report = failed.report().unwrap();
        assert!(report.contains("a..b"), "{report}");
        assert!(report.contains("/v/b.log"), "{report}");

        let still = failed
            .clone()
            .after(Some("a".into()), "c", false, PathBuf::from("/v/c.log"));
        assert_eq!(still.failed, failed.failed, "the smallest range stays");

        let fixed = still.after(Some("a".into()), "d", true, PathBuf::from("/v/d.log"));
        assert_eq!(fixed.verified.as_deref(), Some("d"));
        assert!(fixed.report().is_none());
    }
}
