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
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

use crate::{Context, Result};
use oer_process as process;

/// Rechecks inside the queue before a push gives up; `main` moving there
/// means someone pushes without the queue.
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

/// Directory of the machine-wide push queue; overrides the user's data
/// directory.
pub const QUEUE_ENV: &str = "OER_PUSH_QUEUE";

fn queue_path() -> Result<PathBuf> {
    let root = match std::env::var_os(QUEUE_ENV).filter(|value| !value.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or("HOME is required to locate the push queue")?
            .join("open-esp-radio"),
    };
    fs::create_dir_all(&root)?;
    Ok(root.join("push.lock"))
}

/// Exclusive right to move `main`, held from the recheck to the push. Every
/// session on this machine pushes through it, so `main` cannot move under a
/// check made inside it. The kernel releases it when the process exits.
pub struct PushQueue {
    file: File,
}

impl PushQueue {
    /// Waits for the queue and records who holds it for the next waiter.
    pub fn enter(path: &Path, holder: &str) -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let mut current = String::new();
                file.read_to_string(&mut current)?;
                println!(
                    "push: waiting for the push queue; held by {}",
                    current.trim()
                );
                file.lock()?;
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{holder}")?;
        file.flush()?;
        Ok(Self { file })
    }
}

impl Drop for PushQueue {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        let _ = self.file.unlock();
    }
}

/// A revision that passed `check changed` on top of `base`.
struct Checked {
    base: String,
    head: String,
}

/// Rebase onto the current `origin/main` and check the result; `None` when
/// nothing is left to push.
fn rebase_and_check(ctx: &Context, branch: &str) -> Result<Option<Checked>> {
    let base = origin_main(ctx)?;
    if let Err(error) = process::capture(git(ctx).args(["rebase", "--quiet", "origin/main"])) {
        let _ = process::capture(git(ctx).args(["rebase", "--abort"]));
        return Err(
            format!("push: rebase onto origin/main failed; resolve it by hand: {error}").into(),
        );
    }
    let head = text(git(ctx).args(["rev-parse", "HEAD"]))?;
    if head == base {
        println!("push: nothing to push; {branch} is origin/main");
        return Ok(None);
    }
    println!(
        "push: checking {} on origin/main {}",
        &head[..12],
        &base[..12]
    );
    process::run(ctx.cargo().args(["xtask", "check", "changed"]))
        .map_err(|error| format!("push: check changed failed; nothing pushed: {error}"))?;
    Ok(Some(Checked { base, head }))
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
    // The first check runs outside the queue, so other sessions keep pushing
    // meanwhile; most pushes then find `main` where the check left it.
    let Some(mut checked) = rebase_and_check(ctx, &branch)? else {
        return Ok(());
    };
    let queue = PushQueue::enter(
        &queue_path()?,
        &format!(
            "{} ({branch} {}), pid {}",
            ctx.root.display(),
            &checked.head[..12],
            std::process::id()
        ),
    )?;
    for attempt in 1..=ATTEMPTS {
        let base = origin_main(ctx)?;
        if base == checked.base {
            match process::capture(git(ctx).args(["push", "--quiet", "origin", "HEAD:main"])) {
                Ok(_) => {
                    println!("push: pushed {} to main", &checked.head[..12]);
                    // `main` is final; the next push need not wait for the
                    // reinstall.
                    drop(queue);
                    let changed =
                        text(git(ctx).args(["diff", "--name-only", &base, &checked.head]))?;
                    if touches_stand_tooling(&changed.lines().collect::<Vec<_>>()) {
                        println!("push: the stand tooling changed; reinstalling oer-stand");
                        process::run(ctx.cargo().args(["xtask", "stand-install"]))?;
                    }
                    return Ok(());
                }
                Err(error) => println!("push: rejected ({error})"),
            }
        } else {
            println!(
                "push: origin/main moved to {} during the check; checking again inside the push queue ({attempt}/{ATTEMPTS})",
                &base[..12]
            );
        }
        match rebase_and_check(ctx, &branch)? {
            Some(next) => checked = next,
            None => return Ok(()),
        }
    }
    Err(format!(
        "push: origin/main moved {ATTEMPTS} times while this push held the queue; something pushes around `cargo xtask push`"
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_admits_one_holder_and_names_it_to_the_next() {
        use std::{sync::mpsc, thread, time::Duration};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push.lock");
        let first = PushQueue::enter(&path, "first holder").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().trim(), "first holder");
        let (entered, admitted) = mpsc::channel();
        let waiter_path = path.clone();
        let waiter = thread::spawn(move || {
            let queue = PushQueue::enter(&waiter_path, "second holder").unwrap();
            entered.send(()).unwrap();
            drop(queue);
        });
        assert!(admitted.recv_timeout(Duration::from_millis(300)).is_err());
        drop(first);
        admitted.recv_timeout(Duration::from_secs(10)).unwrap();
        waiter.join().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

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
