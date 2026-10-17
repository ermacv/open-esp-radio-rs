//! Push this checkout's branch after the gate passed on exactly the tree that
//! is pushed, and hand it to CI through its pull request.
//!
//! 1. The tree must be `HEAD`: an uncommitted change to a tracked file, or
//!    an untracked Rust source or Cargo manifest or lock that a build would
//!    read, refuses the push, since the gate would check what is not pushed.
//! 2. The branch is not `main`: `main` changes only through pull requests,
//!    which CI checks and a person merges.
//! 3. The branch contains the current `origin/main`: the push fetches it
//!    before the gate, and again after the gate and the review right before
//!    `git push` (step 6), and refuses a branch that lacks any of its
//!    commits, naming them and the rebase that brings them in
//!    ([`current_main`], [`checked`]), so a pull request is never opened or
//!    updated against a stale `main`. A failed fetch refuses the push too.
//! 4. The registry's fast checks ([`crate::registry`]) check what `HEAD`'s
//!    commits change against `origin/main`, their merge base.
//! 5. The local Claude review ([`crate::review`]) of the same diff has no
//!    blocking finding (`reviewed`): it is the only review that blocks;
//!    the one in CI reviews the pull request once and only files its
//!    findings as issues. Its wording findings do not block: once the pull
//!    request exists, they are filed as `review: …` issues of the branch,
//!    and a failed filing fails the push. `--skip-review REASON` is the only
//!    way to push without it and records the reason on the pull request.
//! 6. Push the branch and open its pull request as a draft when it has none
//!    (`create_pull_request`). The push enables no auto-merge: a person
//!    marks the pull request ready and merges it.
//!
//! The push never waits for CI and never rebases: the developer rebases a
//! branch that `main` outran and pushes again. Since the gate and the
//! review take minutes, a `main` that moved meanwhile refuses the push too;
//! an unchanged diff reuses its review verdict after the rebase.
//!
//! A branch the developer rebased replaces its remote copy with
//! `--force-with-lease` bound to the copy this push saw ([`Update::Rewrite`]), so a commit pushed
//! there by anyone else in the meantime refuses the push instead of being
//! lost.

use std::path::{Path, PathBuf};

use oer_process::{self as process, git};

use crate::{Result, gate, registry, review};
use oer_process::Checkout;

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

/// How many of the commits a branch lacks a refusal names one by one.
const NAMED_MISSING: usize = 20;

/// Why a branch that lacks the `missing` commits of `origin/main` (one
/// `<hash> <subject>` line each, newest first) cannot be pushed, if it lacks
/// any.
pub fn behind(missing: &[String]) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    let mut named: String = missing
        .iter()
        .take(NAMED_MISSING)
        .map(|commit| format!("\n  {commit}"))
        .collect();
    if missing.len() > NAMED_MISSING {
        named.push_str(&format!("\n  … and {} more", missing.len() - NAMED_MISSING));
    }
    Some(format!(
        "the branch is behind origin/{MAIN} by {} commit(s); nothing pushed. \
         Run `git rebase origin/{MAIN}`, then push again. It lacks:{named}",
        missing.len()
    ))
}

/// Fetch `main` from `origin` into `origin/main` and return its commit when
/// the commit `head` contains it. A failed fetch, or a `head` that lacks a
/// commit of it ([`behind`]), is an error: the push never builds on a stale
/// `main`.
pub fn current_main(root: &Path, head: &str) -> Result<String> {
    let refspec = format!("+refs/heads/{MAIN}:refs/remotes/origin/{MAIN}");
    git::output(root, ["fetch", "--quiet", "origin", &refspec])
        .map_err(|error| format!("push: fetching origin/{MAIN} failed; nothing pushed: {error}"))?;
    let main = git::text(
        root,
        [
            "rev-parse",
            "--verify",
            &format!("refs/remotes/origin/{MAIN}^{{commit}}"),
        ],
    )?;
    let missing = git::lines(root, ["log", "--format=%h %s", &format!("{head}..{main}")])?;
    match behind(&missing) {
        Some(reason) => Err(format!("push: {reason}").into()),
        None => Ok(main),
    }
}

/// The commits the push checked: `HEAD` and its merge base with
/// `origin/main`, which is `origin/main` itself, with what the checks
/// returned.
#[derive(Debug)]
pub struct Checked<T> {
    pub base: String,
    pub head: String,
    pub result: T,
}

/// Run `checks` (the gate and the review, handed the merge base and `HEAD`)
/// on a branch that contains the current `origin/main`, then confirm that
/// neither `HEAD` nor `origin/main` moved while they ran, so the push sends
/// exactly what they checked, on the current `main`. `None` when `HEAD` is
/// in `origin/main` and there is nothing to push; `checks` does not run.
pub fn checked<T>(
    root: &Path,
    checks: impl FnOnce(&str, &str) -> Result<T>,
) -> Result<Option<Checked<T>>> {
    let head = git::text(root, ["rev-parse", "HEAD"])?;
    // `head` contains `origin/main`, so that is their merge base.
    let base = current_main(root, &head)?;
    if head == base {
        return Ok(None);
    }
    let result = checks(&base, &head)?;
    // The gate and the review take minutes while the session keeps working:
    // push only the commit they checked.
    let now = git::text(root, ["rev-parse", "HEAD"])?;
    if now != head {
        return Err(format!(
            "push: HEAD moved from {} to {} after it was gated; nothing pushed, push again",
            &head[..head.len().min(12)],
            &now[..now.len().min(12)]
        )
        .into());
    }
    // `main` may have moved meanwhile too: check the commit that is pushed
    // against it again right before the push.
    current_main(root, &head).map_err(|error| {
        format!("{error}\n(checked again before `git push`, after the gate and the review)")
    })?;
    Ok(Some(Checked { base, head, result }))
}

fn gh(ctx: &Checkout, arguments: &[&str]) -> Result<String> {
    let output = process::capture(ctx.command("gh").args(arguments))?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// The local review of a push.
#[derive(Debug)]
enum Reviewed {
    /// `--skip-review`.
    Skipped,
    /// No blocking finding.
    Passed(review::Verdict),
    /// The round's last pass ([`review::PASSES`]) still blocks: the
    /// push proceeds and files its blocking findings.
    Capped(review::Verdict),
}

/// The local review's verdict on a push that `review` reviews; a blocking
/// finding refuses it unless `capped` says the round used its last pass or
/// the verdict let a push through past the cap already.
fn reviewed(
    skip_review: Option<&str>,
    review: impl FnOnce() -> Result<(review::Verdict, PathBuf)>,
    capped: impl FnOnce(&review::Verdict) -> bool,
) -> Result<Reviewed> {
    if skip_review.is_some() {
        return Ok(Reviewed::Skipped);
    }
    let (mut verdict, path) = review().map_err(|error| {
        format!(
            "push: the local review did not complete; nothing pushed: {error}\n\
             Without Claude Code, `cargo xtask push --skip-review \"<reason>\"` pushes \
             unreviewed; CI's review only files findings as issues and blocks nothing"
        )
    })?;
    print!("{}", review::render(&verdict));
    let blocking = verdict.blocking();
    if blocking > 0 && capped(&verdict) {
        println!(
            "push: {}; the push proceeds and files them as issues",
            cap_reason(&verdict)
        );
        // A retried or rebase-only push reusing it proceeds too.
        review::mark_capped(&path, &mut verdict)?;
        return Ok(Reviewed::Capped(verdict));
    }
    if blocking > 0 {
        return Err(format!(
            "push: the local review found {blocking} blocking finding(s); nothing pushed. \
             Fix each one and every place with the same defect, commit and push again \
             (report: {})",
            path.display()
        )
        .into());
    }
    Ok(Reviewed::Passed(verdict))
}

/// The skip reason a capped push records.
fn cap_reason(verdict: &review::Verdict) -> String {
    let titles: Vec<&str> = verdict
        .report
        .findings
        .iter()
        .filter(|finding| finding.blocking())
        .map(|finding| finding.title.as_str())
        .collect();
    format!("review pass cap reached; findings: {}", titles.join("; "))
}

/// The marks around the pass cap's note in a pull request's body.
const CAP_BEGIN: &str = "<!-- oer-review-cap -->";
const CAP_END: &str = "<!-- /oer-review-cap -->";

/// `body` with the pass cap's note `note` in place of an earlier one.
fn with_cap_note(body: &str, note: &str) -> String {
    let kept = match (body.find(CAP_BEGIN), body.find(CAP_END)) {
        (Some(start), Some(stop)) if start < stop => {
            format!("{}{}", &body[..start], &body[stop + CAP_END.len()..])
        }
        _ => body.to_owned(),
    };
    format!("{}\n\n{CAP_BEGIN}\n{note}\n{CAP_END}\n", kept.trim_end())
}

/// The `gh` arguments that open `branch`'s pull request: always a draft,
/// and nothing here or after it enables auto-merge.
fn create_pull_request(branch: &str) -> [&str; 8] {
    [
        "pr",
        "create",
        "--draft",
        "--fill-first",
        "--base",
        MAIN,
        "--head",
        branch,
    ]
}

/// Gate, push, open the pull request; see the module documentation.
pub fn run(ctx: &Checkout, skip_review: Option<&str>) -> Result<()> {
    // Porcelain lines start with their status columns: never trim them.
    let tracked: Vec<String> =
        git::lines(&ctx.root, ["status", "--porcelain", "--untracked-files=no"])?
            .iter()
            .map(|line| line.get(3..).unwrap_or(line).to_owned())
            .collect();
    let untracked = git::lines(&ctx.root, ["ls-files", "--others", "--exclude-standard"])?;
    if let Some(reason) = unpushable(&tracked, &untracked) {
        return Err(format!("push: {reason}").into());
    }
    let branch = git::text(&ctx.root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    if let Some(reason) = unreviewable(&branch) {
        return Err(format!("push: {reason}").into());
    }
    let gated = checked(&ctx.root, |base, head| {
        let change = gate::Change::of(
            ctx,
            gate::committed(ctx, base)?,
            base,
            None,
            registry::Tier::Fast,
        )?;
        println!(
            "push: gating {} on {}: {} files, {} package(s) with dependents",
            &head[..12],
            &base[..12],
            change.files.len(),
            change.affected.len()
        );
        registry::run_change(ctx, &change)
            .map_err(|error| format!("push: the gate failed; nothing pushed: {error}"))?;
        let pushed = review::pushed_head(ctx, &branch)?;
        reviewed(
            skip_review,
            || review::verdict(ctx, &branch, base, head),
            |verdict| verdict.capped || review::capped(ctx, &branch, pushed.as_deref(), base),
        )
    })?;
    let Some(Checked {
        head,
        result: reviewed,
        ..
    }) = gated
    else {
        println!("push: nothing to push; HEAD is in origin/{MAIN}");
        return Ok(());
    };
    let reference = format!("refs/heads/{branch}");
    let remote = git::text(&ctx.root, ["ls-remote", "origin", &reference])?
        .split_whitespace()
        .next()
        .map(str::to_owned);
    let descends = match &remote {
        Some(seen) => {
            git::output(&ctx.root, ["fetch", "--quiet", "origin", &reference])?;
            git::output(&ctx.root, ["merge-base", "--is-ancestor", seen, &head]).is_ok()
        }
        None => true,
    };
    let update = update(remote.as_deref(), descends);
    let mut command = git::command(&ctx.root);
    command.args(["push", "--quiet"]);
    if let Update::Rewrite { seen } = &update {
        command.arg(format!("--force-with-lease={reference}:{seen}"));
    }
    process::capture(command.args(["origin", &format!("{head}:{reference}")]))
        .map_err(|error| format!("push: git push of {branch} failed: {error}"))?;
    // A commit, not a branch, was pushed, so Git set no upstream.
    git::output(
        &ctx.root,
        [
            "branch",
            &format!("--set-upstream-to=origin/{branch}"),
            &branch,
        ],
    )?;
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
        None => gh(ctx, &create_pull_request(&branch))
            .map_err(|error| format!("push: gh pr create failed: {error}"))?,
    };
    // The pull request's number ends its URL.
    let number = url.rsplit('/').next().unwrap_or_default();
    let (verdict, capped) = match &reviewed {
        Reviewed::Skipped => (None, false),
        Reviewed::Passed(verdict) => (Some(verdict), false),
        Reviewed::Capped(verdict) => (Some(verdict), true),
    };
    if let Some(verdict) = verdict {
        let mut report = verdict.wording().unwrap_or_else(|| review::Report {
            summary: verdict.report.summary.clone(),
            findings: Vec::new(),
        });
        if capped {
            report.findings.extend(verdict.blocking_report().findings);
        }
        let filed = review::file_findings(ctx, verdict, Some(report), number, &branch, &head)
            .map_err(|error| {
                format!(
                    "push: filing the review's findings failed; {branch} is pushed, push again: \
                     {error}"
                )
            })?;
        for line in filed {
            println!("push: filed finding {}", line.trim_start_matches("- "));
        }
    }
    let cap = match &reviewed {
        Reviewed::Capped(verdict) => Some(cap_reason(verdict)),
        _ => None,
    };
    if let Some(reason) = &cap {
        let body = gh(
            ctx,
            &["pr", "view", &branch, "--json", "body", "--jq", ".body"],
        )?;
        let note = format!(
            "**Local review pass cap reached** at `{}`: the round's second pass still \
             blocked, so the push proceeded past it, as `--skip-review` would. {reason}; they \
             are filed as `review: …` issues of this branch.",
            &head[..12]
        );
        gh(
            ctx,
            &[
                "pr",
                "edit",
                &branch,
                "--body",
                &with_cap_note(&body, &note),
            ],
        )
        .map_err(|error| format!("push: recording the pass cap on {url} failed: {error}"))?;
    }
    let unreviewed = match (skip_review, &cap) {
        (Some(reason), _) => Some(format!(
            "The local Claude review was skipped for this push: {reason}"
        )),
        (None, Some(reason)) => Some(format!(
            "The local Claude review blocked this push in its round's last pass, and the push \
             went through past it: {reason}"
        )),
        (None, None) => None,
    };
    if let Some(body) = unreviewed {
        gh(ctx, &["pr", "comment", &branch, "--body", &body]).map_err(|error| {
            format!("push: recording the review's outcome on {url} failed: {error}")
        })?;
        println!("push: the review's outcome is recorded on {url}");
    }
    if opened {
        println!("push: opened draft pull request {url}; a person marks it ready and merges it");
    } else {
        println!("push: updated pull request {url}");
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

    fn verdict(severity: &str) -> review::Verdict {
        review::Verdict {
            branch: String::from("fix/a"),
            head: String::from("0123456789abcdef"),
            base: String::from("fedcba9876543210"),
            identity: String::from("id"),
            report: review::Report {
                summary: String::from("Checked the push."),
                findings: vec![review::Finding {
                    severity: severity.to_owned(),
                    path: String::from("tools/xtask/src/push.rs"),
                    line: 1,
                    title: String::from("A finding"),
                    body: String::from("Its body."),
                    rest: serde_json::Map::new(),
                }],
            },
            cost_usd: 0.0,
            mode: review::round::Mode::Full,
            reason: String::new(),
            seconds: 0.0,
            pushed: None,
            capped: false,
        }
    }

    /// Every push is reviewed: the review runs, a blocking finding refuses
    /// it and a wording finding lets it through; only `--skip-review` skips
    /// the review.
    #[test]
    fn every_push_is_reviewed_and_a_blocking_finding_refuses_it() {
        let mut ran = false;
        let error = reviewed(
            None,
            || {
                ran = true;
                Ok((verdict("nit"), PathBuf::from("report.json")))
            },
            |_| false,
        )
        .unwrap_err()
        .to_string();
        assert!(ran);
        assert!(
            error.contains("1 blocking finding(s); nothing pushed"),
            "{error}"
        );
        let passed =
            reviewed(None, || Ok((verdict("wording"), PathBuf::new())), |_| false).unwrap();
        assert!(matches!(passed, Reviewed::Passed(verdict) if verdict.blocking() == 0));
        let skipped = reviewed(
            Some("Claude Code is down"),
            || unreachable!("--skip-review runs no review"),
            |_| unreachable!("--skip-review counts no pass"),
        )
        .unwrap();
        assert!(matches!(skipped, Reviewed::Skipped));
    }

    /// The round's last pass still blocking lets the push through with the
    /// cap's reason, which names the findings it files.
    #[test]
    fn a_capped_round_pushes_with_its_reason() {
        let stored = tempfile::NamedTempFile::new().unwrap();
        let path = stored.path().to_owned();
        let capped = reviewed(None, || Ok((verdict("nit"), path.clone())), |_| true).unwrap();
        // The verdict it went through with is marked, for a retried push.
        let marked: review::Verdict =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(marked.capped);
        let Reviewed::Capped(verdict) = capped else {
            panic!("{capped:?}");
        };
        assert_eq!(
            cap_reason(&verdict),
            "review pass cap reached; findings: A finding"
        );
        assert_eq!(verdict.blocking_report().findings.len(), 1);
        // A passing review is never capped.
        assert!(matches!(
            reviewed(
                None,
                || Ok((verdict_of("wording"), PathBuf::new())),
                |_| true
            )
            .unwrap(),
            Reviewed::Passed(_)
        ));
        // The note replaces an earlier one in the pull request's body.
        let once = with_cap_note("Body.", "first");
        let twice = with_cap_note(&once, "second");
        assert!(
            twice.starts_with("Body.") && twice.contains("second") && !twice.contains("first"),
            "{twice}"
        );
    }

    fn verdict_of(severity: &str) -> review::Verdict {
        verdict(severity)
    }

    /// A push opens its pull request as a draft and asks for no
    /// auto-merge.
    #[test]
    fn a_push_opens_a_draft_and_enables_no_auto_merge() {
        let create = create_pull_request("fix/a");
        assert_eq!(&create[..2], ["pr", "create"]);
        assert!(create.contains(&"--draft"), "{create:?}");
        assert!(!create.contains(&"--auto"), "{create:?}");
        assert_eq!(&create[create.len() - 2..], ["--head", "fix/a"]);
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

    /// `git -C directory arguments…` with a fixed identity, its output.
    fn git_in(directory: &Path, arguments: &[&str]) -> String {
        git::text(
            directory,
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
    }

    fn commit(directory: &Path, subject: &str) -> String {
        git_in(
            directory,
            &["commit", "--quiet", "--allow-empty", "-m", subject],
        );
        git_in(directory, &["rev-parse", "HEAD"])
    }

    /// A bare `origin` whose `main` has one commit, and a clone of it on
    /// branch `change`.
    fn repositories() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let origin = directory.path().join("origin.git");
        let seed = directory.path().join("seed");
        let clone = directory.path().join("clone");
        std::fs::create_dir_all(&seed).unwrap();
        git_in(&seed, &["init", "--quiet", "--initial-branch", MAIN]);
        commit(&seed, "base");
        git_in(
            directory.path(),
            &["clone", "--quiet", "--bare", "seed", "origin.git"],
        );
        git_in(
            directory.path(),
            &["clone", "--quiet", "origin.git", "clone"],
        );
        git_in(&clone, &["switch", "--quiet", "-c", "change"]);
        git_in(
            &seed,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        (directory, seed, clone)
    }

    #[test]
    fn a_branch_on_the_current_main_passes() {
        let (_directory, seed, clone) = repositories();
        commit(&clone, "the change");
        let main = git_in(&seed, &["rev-parse", "HEAD"]);
        assert_eq!(current_main(&clone, "HEAD").unwrap(), main);
        // `main` moved and the branch was rebased onto it.
        let moved = commit(&seed, "upstream");
        git_in(&seed, &["push", "--quiet", "origin", MAIN]);
        git_in(&clone, &["fetch", "--quiet", "origin"]);
        git_in(&clone, &["rebase", "--quiet", "origin/main"]);
        assert_eq!(current_main(&clone, "HEAD").unwrap(), moved);
    }

    #[test]
    fn a_main_that_moves_while_the_checks_run_refuses_the_push() {
        let (_directory, seed, clone) = repositories();
        let head = commit(&clone, "the change");
        let main = git_in(&seed, &["rev-parse", "HEAD"]);
        let error = checked(&clone, |base, checked| {
            assert_eq!((base, checked), (main.as_str(), head.as_str()));
            // A pull request merges while the gate and the review run.
            commit(&seed, "merged meanwhile");
            git_in(&seed, &["push", "--quiet", "origin", MAIN]);
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("behind origin/main by 1 commit(s)"),
            "{error}"
        );
        assert!(error.contains("merged meanwhile"), "{error}");
        assert!(error.contains("checked again before `git push`"), "{error}");
    }

    #[test]
    fn the_pushed_commit_is_checked_not_the_live_head() {
        let (_directory, seed, clone) = repositories();
        let pushed = commit(&clone, "the change");
        commit(&seed, "upstream");
        git_in(&seed, &["push", "--quiet", "origin", MAIN]);
        // The session rebased meanwhile: the live `HEAD` is current, the
        // commit the checks passed is not.
        git_in(&clone, &["fetch", "--quiet", "origin"]);
        git_in(&clone, &["rebase", "--quiet", "origin/main"]);
        current_main(&clone, "HEAD").unwrap();
        let error = current_main(&clone, &pushed).unwrap_err().to_string();
        assert!(
            error.contains("behind origin/main by 1 commit(s)"),
            "{error}"
        );
    }

    #[test]
    fn the_push_sends_exactly_what_the_checks_passed() {
        let (_directory, seed, clone) = repositories();
        let head = commit(&clone, "the change");
        let main = git_in(&seed, &["rev-parse", "HEAD"]);
        let passed = checked(&clone, |_, _| Ok(7)).unwrap().unwrap();
        assert_eq!((passed.base, passed.head, passed.result), (main, head, 7));
        // A commit made while the checks ran is not what they checked.
        let error = checked(&clone, |_, _| {
            commit(&clone, "made meanwhile");
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("HEAD moved"), "{error}");
    }

    #[test]
    fn nothing_is_checked_when_head_is_in_main() {
        let (_directory, _seed, clone) = repositories();
        let checked = checked(&clone, |_, _| -> Result<()> { panic!("checked") }).unwrap();
        assert!(checked.is_none());
    }

    #[test]
    fn a_branch_behind_main_is_refused_naming_what_it_lacks() {
        let (_directory, seed, clone) = repositories();
        commit(&clone, "the change");
        commit(&seed, "first upstream");
        commit(&seed, "second upstream");
        git_in(&seed, &["push", "--quiet", "origin", MAIN]);
        // The clone has not fetched: the push must fetch to see them.
        let error = current_main(&clone, "HEAD").unwrap_err().to_string();
        assert!(
            error.contains("behind origin/main by 2 commit(s)"),
            "{error}"
        );
        assert!(error.contains("git rebase origin/main"), "{error}");
        assert!(
            error.find("second upstream").unwrap() < error.find("first upstream").unwrap(),
            "{error}"
        );
        assert!(!error.contains("the change"), "{error}");
    }

    #[test]
    fn a_failed_fetch_refuses_the_push() {
        let (directory, _seed, clone) = repositories();
        commit(&clone, "the change");
        std::fs::remove_dir_all(directory.path().join("origin.git")).unwrap();
        let error = current_main(&clone, "HEAD").unwrap_err().to_string();
        assert!(error.contains("fetching origin/main failed"), "{error}");
    }

    #[test]
    fn a_long_list_of_missing_commits_is_cut() {
        let missing: Vec<String> = (0..25).map(|n| format!("abc{n} commit {n}")).collect();
        let reason = behind(&missing).unwrap();
        assert!(reason.contains("abc19 commit 19"), "{reason}");
        assert!(!reason.contains("abc20"), "{reason}");
        assert!(reason.contains("and 5 more"), "{reason}");
        assert_eq!(behind(&[]), None);
    }

    #[test]
    fn main_changes_only_through_pull_requests() {
        assert!(unreviewable("main").unwrap().contains("pull requests"));
        assert!(unreviewable("HEAD").unwrap().contains("detached"));
        assert_eq!(unreviewable("fix-publication"), None);
    }
}
