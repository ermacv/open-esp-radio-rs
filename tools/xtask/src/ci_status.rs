//! The state of GitHub CI on `main`, as `check changed`, `push` and `cargo
//! xtask ci-status` report it; the session start hook runs `ci-status`
//! through the built `oer-xtask` executable, so this is the one reader.
//!
//! CI verifies `main` after every push: the source checks, both final HIL
//! images with their audits, and every image class once a night. A failure
//! there does not block a push; it is printed on every `check changed` and
//! `push` until a later run of the same workflow passes, so whoever pushed the
//! failing commit, or anyone who sees it first, fixes it before other work.
//!
//! The Claude review in CI never blocks a merge: it files each finding as an
//! issue titled `review: …`, which names the reviewed branch. The open ones
//! are listed the same way: those of this checkout's branch one by one, to fix
//! next; those no session fixed within a day and nobody claimed (assigned)
//! one by one to any session, to claim before new work,
//! since agents push and move on before the review reports; the rest as a
//! count. An open pull request whose review ended in error (it did not
//! complete, or its findings were not filed) is listed for a `@claude review`.

use std::{collections::BTreeSet, path::Path};

use oer_process::Checkout;
use oer_process::{self as process, git};
use serde::Deserialize;

/// A workflow run as `gh run list --json` reports it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub workflow_name: String,
    pub head_sha: String,
    pub status: RunStatus,
    #[serde(deserialize_with = "conclusion")]
    pub conclusion: Option<Conclusion>,
    pub url: String,
    pub database_id: u64,
    /// RFC 3339 in UTC, so the text orders as the time does.
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    InProgress,
    Queued,
    Pending,
    Requested,
    Waiting,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Conclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
    Neutral,
    Stale,
    StartupFailure,
    #[serde(other)]
    Other,
}

impl Conclusion {
    fn failed(self) -> bool {
        matches!(self, Self::Failure | Self::TimedOut | Self::StartupFailure)
    }

    /// Whether the run judged its commit. A run cancelled because a newer
    /// push superseded it, or skipped, says nothing about the code.
    fn is_verdict(self) -> bool {
        self == Self::Success || self.failed()
    }
}

/// `gh` reports an unfinished run's conclusion as an empty string.
fn conclusion<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Conclusion>, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty() {
        return Ok(None);
    }
    serde_json::from_value(serde_json::Value::String(text))
        .map(Some)
        .map_err(serde::de::Error::custom)
}

/// The names of the workflows in `.github/workflows` of `root`: the
/// top-level `name:` of each file.
fn workflows(root: &Path) -> std::io::Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(root.join(".github/workflows"))? {
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "yml" || extension == "yaml")
        {
            continue;
        }
        if let Some(name) = std::fs::read_to_string(&path)?
            .lines()
            .find_map(|line| line.strip_prefix("name:"))
        {
            names.insert(name.trim().trim_matches(['\'', '"']).to_owned());
        }
    }
    Ok(names)
}

/// For each workflow of `workflows`, its newest run in `runs` that judged its
/// commit, when that run failed. Cancelled and skipped runs are passed over,
/// and so are the runs of a workflow the tree no longer has.
pub fn failures<'a>(runs: &'a [Run], workflows: &BTreeSet<String>) -> Vec<&'a Run> {
    let mut judged = runs
        .iter()
        .filter(|run| {
            workflows.contains(&run.workflow_name)
                && run.status == RunStatus::Completed
                && run.conclusion.is_some_and(Conclusion::is_verdict)
        })
        .collect::<Vec<_>>();
    judged.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut seen = std::collections::BTreeSet::new();
    judged
        .into_iter()
        .filter(|run| seen.insert(run.workflow_name.as_str()))
        .filter(|run| run.conclusion.is_some_and(Conclusion::failed))
        .collect()
}

#[derive(Deserialize)]
struct Jobs {
    jobs: Vec<Job>,
}

#[derive(Deserialize)]
struct Job {
    name: String,
    #[serde(deserialize_with = "conclusion")]
    conclusion: Option<Conclusion>,
}

/// One line per workflow whose newest finished run on `main` failed, naming
/// the failed jobs; empty when CI is green. An error names why `gh` could
/// not tell.
pub fn report(ctx: &Checkout) -> Result<Vec<String>, String> {
    report_with(ctx, "gh")
}

fn report_with(ctx: &Checkout, gh: &str) -> Result<Vec<String>, String> {
    let workflows =
        workflows(&ctx.root).map_err(|error| format!("cannot read .github/workflows: {error}"))?;
    let mut runs = Vec::new();
    // Issue/label events can fill a repository-wide page without any
    // source CI runs. Give every configured workflow its own window.
    for workflow in &workflows {
        let output = process::capture(ctx.command(gh).args([
            "run",
            "list",
            "--workflow",
            workflow.as_str(),
            "--branch",
            "main",
            "--limit",
            "30",
            "--json",
            "workflowName,headSha,status,conclusion,url,databaseId,createdAt",
        ]))
        .map_err(|error| match error.downcast_ref::<std::io::Error>() {
            Some(io) if io.kind() == std::io::ErrorKind::NotFound => {
                "`gh` is not installed".to_owned()
            }
            _ => error
                .to_string()
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("`gh run list` failed")
                .trim()
                .to_owned(),
        })?;
        runs.extend(
            serde_json::from_slice::<Vec<Run>>(&output.stdout)
                .map_err(|error| format!("`gh run list` printed no run list: {error}"))?,
        );
    }
    Ok(failures(&runs, &workflows)
        .into_iter()
        .map(|run| {
            let jobs = process::capture(ctx.command(gh).args([
                "run",
                "view",
                &run.database_id.to_string(),
                "--json",
                "jobs",
            ]))
            .ok()
            .and_then(|output| serde_json::from_slice::<Jobs>(&output.stdout).ok())
            .map(|jobs| {
                jobs.jobs
                    .into_iter()
                    .filter(|job| job.conclusion.is_some_and(Conclusion::failed))
                    .map(|job| job.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
            format!(
                "CI on main is red: {} failed at {}{}; fix it before other work: {}",
                run.workflow_name,
                &run.head_sha[..run.head_sha.len().min(12)],
                if jobs.is_empty() {
                    String::new()
                } else {
                    format!(" ({jobs})")
                },
                run.url
            )
        })
        .collect())
}

/// The title prefix of an issue the Claude review files for a finding.
const FINDING_TITLE: &str = "review: ";
/// The body line that names the branch a finding was reviewed on.
const FINDING_BRANCH: &str = "<!-- claude-review-branch: ";

/// The author `gh` reports for issues the review workflow files.
const WORKFLOW_AUTHOR: &str = "app/github-actions";

/// An open issue as `gh issue list --json` reports it.
#[derive(Deserialize)]
struct Issue {
    number: u64,
    title: String,
    url: String,
    #[serde(default)]
    body: String,
    author: Author,
    #[serde(rename = "createdAt")]
    created_at: String,
    /// A session claims a finding by assigning it.
    #[serde(default)]
    assignees: Vec<Author>,
}

#[derive(Deserialize)]
struct Author {
    login: String,
}

/// After this long a finding whose branch's session never fixed it is
/// anyone's: agents push and move on before the review in CI reports.
const UNOWNED_AFTER_SECONDS: u64 = 24 * 60 * 60;
/// At most this many unowned findings are named; the rest are counted.
const UNOWNED_NAMED: usize = 3;

/// The Unix time of a `gh` timestamp such as `2026-10-10T14:42:28Z`.
fn unix_seconds(timestamp: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| timestamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

/// The branch a session that claimed finding `number` fixes it on.
pub fn claim_branch(number: u64) -> String {
    format!("review-{number}")
}

/// What decides how a finding is listed to this checkout.
struct View<'a> {
    /// This checkout's branch.
    branch: Option<&'a str>,
    /// The authors whose `review: …` issues are findings.
    trusted: &'a [&'a str],
    /// The current Unix time.
    now: u64,
}

/// The owner of the GitHub repository at `url`, `origin`'s URL.
fn owner(url: &str) -> Option<&str> {
    let path = url
        .split_once("github.com")?
        .1
        .trim_start_matches([':', '/']);
    path.split('/').next().filter(|owner| !owner.is_empty())
}

impl Issue {
    fn branch(&self) -> Option<&str> {
        self.body
            .lines()
            .find_map(|line| line.strip_prefix(FINDING_BRANCH)?.strip_suffix(" -->"))
    }
}

/// The lines that list open review findings to a checkout:
///
/// - each finding of its branch, reviewed there or fixed there under its
///   [`claim_branch`] name, to fix before other work;
/// - unowned findings, older than [`UNOWNED_AFTER_SECONDS`] and assigned to
///   nobody, oldest first, for any session to claim before new work;
/// - a count of the rest.
///
/// Only issues of a `trusted` author are findings, as for the workflow's
/// deduplication (`.github/scripts/review_findings.py`): anyone can open an
/// issue, and these lines tell a session what to do first.
fn review_findings(issues: &[Issue], view: &View) -> Vec<String> {
    let mut findings: Vec<&Issue> = issues
        .iter()
        .filter(|issue| issue.title.starts_with(FINDING_TITLE))
        .filter(|issue| view.trusted.contains(&issue.author.login.as_str()))
        .collect();
    findings.sort_by_key(|issue| issue.number);
    let title = |issue: &Issue| issue.title.trim_start_matches(FINDING_TITLE).to_owned();
    let mut lines = Vec::new();
    let mut unowned = Vec::new();
    let mut others = Vec::new();
    for issue in findings {
        let claim = claim_branch(issue.number);
        let claims = |branch: &str| {
            branch == claim
                || branch
                    .strip_suffix(claim.as_str())
                    .is_some_and(|p| p.ends_with('/'))
        };
        let mine = view
            .branch
            .is_some_and(|branch| issue.branch() == Some(branch) || claims(branch));
        let claimed = !issue.assignees.is_empty();
        let old = unix_seconds(&issue.created_at)
            .is_some_and(|created| view.now.saturating_sub(created) >= UNOWNED_AFTER_SECONDS);
        if mine {
            lines.push(format!(
                "Claude review finding #{} of this branch; fix it before other work: {}: {}",
                issue.number,
                title(issue),
                issue.url
            ));
        } else if old && !claimed {
            unowned.push(issue);
        } else {
            others.push(issue);
        }
    }
    for issue in unowned.iter().take(UNOWNED_NAMED) {
        lines.push(format!(
            "Claude review finding #{} has no owner for a day; claim it before new work \
             (`gh issue edit {} --add-assignee @me`) and fix it on branch {}: {}: {}",
            issue.number,
            issue.number,
            claim_branch(issue.number),
            title(issue),
            issue.url
        ));
    }
    others.extend(unowned.iter().skip(UNOWNED_NAMED));
    if !others.is_empty() {
        let numbers: Vec<String> = others
            .iter()
            .map(|issue| format!("#{}", issue.number))
            .collect();
        lines.push(format!(
            "{} other open Claude review finding(s): {}",
            others.len(),
            numbers.join(", ")
        ));
    }
    lines
}

/// [`review_findings`] of the repository's open issues.
fn open_findings(ctx: &Checkout) -> Result<Vec<String>, String> {
    let output = process::capture(ctx.command("gh").args([
        "issue",
        "list",
        "--state",
        "open",
        "--search",
        "\"review:\" in:title",
        "--limit",
        "200",
        "--json",
        "number,title,url,body,author,createdAt,assignees",
    ]))
    .map_err(|error| {
        error
            .to_string()
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("`gh issue list` failed")
            .trim()
            .to_owned()
    })?;
    let issues = serde_json::from_slice::<Vec<Issue>>(&output.stdout)
        .map_err(|error| format!("`gh issue list` printed no issue list: {error}"))?;
    let branch = git::text(&ctx.root, ["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    // The workflow and the repository owner, who files findings by hand.
    let origin = git::text(&ctx.root, ["remote", "get-url", "origin"]).unwrap_or_default();
    let trusted: Vec<&str> = std::iter::once(WORKFLOW_AUTHOR)
        .chain(owner(&origin))
        .collect();
    let view = View {
        branch: branch.as_deref(),
        trusted: &trusted,
        now: oer_durable::unix_seconds(),
    };
    Ok(review_findings(&issues, &view))
}

/// The status the Claude review workflow sets on a pull request's head.
const REVIEW_STATUS: &str = "claude-runtime-review";

/// An open pull request as `gh pr list --json` reports it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequest {
    number: u64,
    url: String,
    head_ref_name: String,
    status_check_rollup: Vec<serde_json::Value>,
}

/// One line per open pull request whose Claude review ended in `error`: it
/// did not complete, or its findings were not filed as issues, so they reach
/// no session until a `@claude review` comment reviews it again. The session
/// on that branch (`branch`, this checkout's) asks for it.
fn unreviewed(pulls: &[PullRequest], branch: Option<&str>) -> Vec<String> {
    pulls
        .iter()
        .filter(|pull| {
            pull.status_check_rollup
                .iter()
                .any(|check| check["context"] == REVIEW_STATUS && check["state"] == "ERROR")
        })
        .map(|pull| {
            let (whose, who) = if branch == Some(pull.head_ref_name.as_str()) {
                (" of this branch", "comment")
            } else {
                ("", "the session on its branch comments")
            };
            format!(
                "PR #{} ({}){whose} has no Claude review: it failed or its findings were not \
                 filed; {who} `@claude review` on it: {}",
                pull.number, pull.head_ref_name, pull.url
            )
        })
        .collect()
}

/// [`unreviewed`] of the repository's open pull requests.
fn unreviewed_pulls(ctx: &Checkout) -> Result<Vec<String>, String> {
    let output = process::capture(ctx.command("gh").args([
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "100",
        "--json",
        "number,url,headRefName,statusCheckRollup",
    ]))
    .map_err(|error| {
        error
            .to_string()
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("`gh pr list` failed")
            .trim()
            .to_owned()
    })?;
    let pulls = serde_json::from_slice::<Vec<PullRequest>>(&output.stdout)
        .map_err(|error| format!("`gh pr list` printed no pull request list: {error}"))?;
    let branch = git::text(&ctx.root, ["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    Ok(unreviewed(&pulls, branch.as_deref()))
}

/// Prints [`report`]'s lines, or one warning line when CI's state is
/// unknown, then the open Claude review findings and the pull requests whose
/// review failed, prefixed with `label`.
pub fn print(ctx: &Checkout, label: &str) {
    match report(ctx) {
        Ok(failures) if failures.is_empty() => println!("{label}: CI on main is green"),
        Ok(failures) => {
            for failure in failures {
                println!("{label}: {failure}");
            }
        }
        Err(reason) => println!("{label}: warning: the state of CI on main is unknown: {reason}"),
    }
    match open_findings(ctx) {
        Ok(findings) => {
            for line in findings {
                println!("{label}: {line}");
            }
        }
        Err(reason) => {
            println!("{label}: warning: open Claude review findings are unknown: {reason}");
        }
    }
    match unreviewed_pulls(ctx) {
        Ok(pulls) => {
            for line in pulls {
                println!("{label}: {line}");
            }
        }
        Err(reason) => {
            println!(
                "{label}: warning: pull requests without a Claude review are unknown: {reason}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(json: &str) -> Vec<Run> {
        serde_json::from_str(json).unwrap()
    }

    fn current() -> BTreeSet<String> {
        ["CI", "Documentation", "Nightly"].map(String::from).into()
    }

    #[test]
    fn a_removed_workflow_never_reads_as_red() {
        let runs = runs(
            r#"[
            {"workflowName":"Firmware matrix","headSha":"c0","status":"completed","conclusion":"failure","url":"f","databaseId":1,"createdAt":"2026-10-02T08:00:00Z"},
            {"workflowName":"CI","headSha":"c1","status":"completed","conclusion":"success","url":"u","databaseId":2,"createdAt":"2026-10-02T07:00:00Z"}
        ]"#,
        );
        assert!(failures(&runs, &current()).is_empty());
    }

    #[test]
    fn workflows_are_named_by_their_files() {
        let directory = tempfile::tempdir().unwrap();
        let workflows_directory = directory.path().join(".github/workflows");
        std::fs::create_dir_all(&workflows_directory).unwrap();
        std::fs::write(
            workflows_directory.join("ci.yml"),
            "name: CI\non:\n  push:\n",
        )
        .unwrap();
        std::fs::write(
            workflows_directory.join("docs.yaml"),
            "# Docs\nname: 'Documentation'\njobs:\n  build:\n    name: inner\n",
        )
        .unwrap();
        std::fs::write(
            workflows_directory.join("notes.md"),
            "name: Not a workflow\n",
        )
        .unwrap();
        assert_eq!(
            workflows(directory.path()).unwrap(),
            ["CI", "Documentation"].map(String::from).into()
        );
    }

    #[test]
    fn only_the_newest_finished_run_of_each_workflow_counts() {
        let runs = runs(
            r#"[
            {"workflowName":"CI","headSha":"c3","status":"in_progress","conclusion":"","url":"u3","databaseId":3,"createdAt":"2026-09-29T03:00:00Z"},
            {"workflowName":"CI","headSha":"c2","status":"completed","conclusion":"failure","url":"u2","databaseId":2,"createdAt":"2026-09-29T02:00:00Z"},
            {"workflowName":"CI","headSha":"c1","status":"completed","conclusion":"success","url":"u1","databaseId":1,"createdAt":"2026-09-29T01:00:00Z"},
            {"workflowName":"Documentation","headSha":"c2","status":"completed","conclusion":"success","url":"d2","databaseId":5,"createdAt":"2026-09-29T05:00:00Z"},
            {"workflowName":"Documentation","headSha":"c1","status":"completed","conclusion":"failure","url":"d1","databaseId":4,"createdAt":"2026-09-29T04:00:00Z"},
            {"workflowName":"Firmware matrix","headSha":"c0","status":"completed","conclusion":"cancelled","url":"f","databaseId":6,"createdAt":"2026-09-29T06:00:00Z"}
        ]"#,
        );
        let failed = failures(&runs, &current());
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].workflow_name, "CI");
        assert_eq!(failed[0].head_sha, "c2");
    }

    #[test]
    fn a_superseded_run_does_not_hide_the_last_verdict() {
        // gh's order is not relied on: the newest verdict wins by time.
        let runs = runs(
            r#"[
            {"workflowName":"CI","headSha":"old","status":"completed","conclusion":"success","url":"u1","databaseId":1,"createdAt":"2026-09-29T01:00:00Z"},
            {"workflowName":"CI","headSha":"new","status":"completed","conclusion":"cancelled","url":"u3","databaseId":3,"createdAt":"2026-09-29T03:00:00Z"},
            {"workflowName":"CI","headSha":"mid","status":"completed","conclusion":"failure","url":"u2","databaseId":2,"createdAt":"2026-09-29T02:00:00Z"}
        ]"#,
        );
        let failed = failures(&runs, &current());
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].head_sha, "mid");
    }

    #[test]
    fn a_missing_gh_is_reported_not_taken_for_green() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::create_dir_all(directory.path().join(".github/workflows")).unwrap();
        std::fs::write(
            directory.path().join(".github/workflows/ci.yml"),
            "name: CI\n",
        )
        .unwrap();
        let ctx = Checkout::new(directory.path()).unwrap();
        assert_eq!(
            report_with(&ctx, "oer-gh-that-does-not-exist"),
            Err("`gh` is not installed".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn frequent_metadata_runs_do_not_hide_a_source_ci_failure() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        let workflow_dir = directory.path().join(".github/workflows");
        std::fs::create_dir_all(&workflow_dir).unwrap();
        for (file, name) in [("ci.yml", "CI"), ("issue-labels.yml", "Issue labels")] {
            std::fs::write(workflow_dir.join(file), format!("name: {name}\n")).unwrap();
        }
        let source = serde_json::json!([{
            "workflowName": "CI", "headSha": "bad", "status": "completed",
            "conclusion": "failure", "url": "ci-url", "databaseId": 1,
            "createdAt": "2026-10-06T10:00:00Z",
        }]);
        let metadata: Vec<_> = (0..30)
            .map(|index| {
                serde_json::json!({
                    "workflowName": "Issue labels", "headSha": "head", "status": "completed",
                    "conclusion": "success", "url": "metadata-url", "databaseId": index + 2,
                    "createdAt": "2026-10-06T11:00:00Z",
                })
            })
            .collect();
        let gh = directory.path().join("gh");
        std::fs::write(
            &gh,
            format!(
                r#"#!/bin/sh
case "$1 $2" in
  "run list")
    case "$3:$4" in
      "--workflow:CI") printf '%s\n' '{source}' ;;
      "--workflow:Issue labels") printf '%s\n' '{metadata}' ;;
      *) exit 2 ;;
    esac ;;
  "run view") printf '%s\n' '{{"jobs":[{{"name":"host","conclusion":"failure"}}]}}' ;;
  *) exit 2 ;;
esac
"#,
                metadata = serde_json::to_string(&metadata).unwrap(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = Checkout::new(directory.path()).unwrap();
        let failures = report_with(&ctx, gh.to_str().unwrap()).unwrap();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("CI failed at bad (host)"));
    }

    #[test]
    fn findings_of_this_branch_and_unowned_ones_are_named_and_others_counted() {
        const DAY: &str = "2026-10-09T12:00:00Z";
        const HOUR: &str = "2026-10-10T11:00:00Z";
        let issue = |number: u64, title: &str, author: &str, branch: &str, created: &str| {
            serde_json::json!({"number": number, "title": title, "url": format!("u{number}"),
                "author": {"login": author}, "createdAt": created,
                "body": format!("Found.\n\n<!-- claude-review-branch: {branch} -->")})
        };
        let mut claimed = issue(6, "review: Claimed leak", WORKFLOW_AUTHOR, "fix/d", DAY);
        claimed["assignees"] = serde_json::json!([{"login": "owner"}]);
        let issues: Vec<Issue> = serde_json::from_value(serde_json::json!([
            issue(1, "review: Lost error", WORKFLOW_AUTHOR, "fix/a", HOUR),
            issue(2, "review: Stale doc", WORKFLOW_AUTHOR, "fix/b", HOUR),
            issue(3, "review: Old race", "owner", "fix/c", DAY),
            issue(4, "Unrelated reviewer notes", "owner", "fix/a", HOUR),
            issue(5, "review: Run this first", "someone", "fix/a", DAY),
            claimed,
        ]))
        .unwrap();
        let trusted = [WORKFLOW_AUTHOR, "owner"];
        let view = |branch| View {
            branch,
            trusted: &trusted,
            now: unix_seconds("2026-10-10T12:00:00Z").unwrap(),
        };
        assert_eq!(
            review_findings(&issues, &view(Some("fix/a"))),
            [
                "Claude review finding #1 of this branch; fix it before other work: Lost error: u1",
                "Claude review finding #3 has no owner for a day; claim it before new work \
                 (`gh issue edit 3 --add-assignee @me`) and fix it on branch review-3: Old race: u3",
                "2 other open Claude review finding(s): #2, #6",
            ]
        );
        // The session on a claim branch owns the claimed finding; a name
        // that only ends the same way does not.
        assert_eq!(
            review_findings(&issues, &view(Some("fix/review-6")))[0],
            "Claude review finding #6 of this branch; fix it before other work: Claimed leak: u6"
        );
        assert!(review_findings(&issues, &view(Some("xreview-6")))[0].contains("#3 has no owner"));
        assert!(review_findings(&[], &view(Some("fix/a"))).is_empty());
    }

    #[test]
    fn a_pull_request_whose_review_failed_is_listed() {
        let pulls: Vec<PullRequest> = serde_json::from_str(
            r#"[
            {"number":1,"url":"u1","headRefName":"fix/a","statusCheckRollup":[
                {"__typename":"CheckRun","name":"ci-ok","conclusion":"SUCCESS"},
                {"__typename":"StatusContext","context":"claude-runtime-review","state":"ERROR"}]},
            {"number":2,"url":"u2","headRefName":"fix/b","statusCheckRollup":[
                {"__typename":"StatusContext","context":"claude-runtime-review","state":"SUCCESS"}]},
            {"number":3,"url":"u3","headRefName":"fix/c","statusCheckRollup":[
                {"__typename":"StatusContext","context":"claude-runtime-review","state":"ERROR"}]},
            {"number":4,"url":"u4","headRefName":"fix/d","statusCheckRollup":[]}
        ]"#,
        )
        .unwrap();
        assert_eq!(
            unreviewed(&pulls, Some("fix/a")),
            [
                "PR #1 (fix/a) of this branch has no Claude review: it failed or its findings \
                 were not filed; comment `@claude review` on it: u1",
                "PR #3 (fix/c) has no Claude review: it failed or its findings were not filed; \
                 the session on its branch comments `@claude review` on it: u3",
            ]
        );
    }

    #[test]
    fn timestamps_are_read_as_unix_seconds() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("2026-10-10T14:42:28Z"), Some(1_791_643_348));
        assert_eq!(unix_seconds("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(unix_seconds("not a time"), None);
    }

    #[test]
    fn the_owner_is_read_from_either_url_form() {
        assert_eq!(
            owner("https://github.com/ermacv/open-esp-radio-rs"),
            Some("ermacv")
        );
        assert_eq!(
            owner("git@github.com:ermacv/open-esp-radio-rs.git"),
            Some("ermacv")
        );
        assert_eq!(owner("/srv/mirror.git"), None);
    }

    #[test]
    fn an_unknown_status_or_conclusion_still_parses() {
        let runs = runs(
            r#"[{"workflowName":"CI","headSha":"c","status":"brand_new","conclusion":"surprise","url":"u","databaseId":1,"createdAt":"2026-09-29T01:00:00Z"}]"#,
        );
        assert_eq!(runs[0].status, RunStatus::Other);
        assert_eq!(runs[0].conclusion, Some(Conclusion::Other));
        assert!(failures(&runs, &current()).is_empty());
    }
}
