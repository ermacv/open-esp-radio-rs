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
//! next, the others as a count.

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

/// An open issue as `gh issue list --json` reports it.
#[derive(Deserialize)]
struct Issue {
    number: u64,
    title: String,
    url: String,
    #[serde(default)]
    body: String,
}

impl Issue {
    fn branch(&self) -> Option<&str> {
        self.body
            .lines()
            .find_map(|line| line.strip_prefix(FINDING_BRANCH)?.strip_suffix(" -->"))
    }
}

/// One line per open review finding of `branch` (this checkout's), to fix
/// before other work, then one line counting every other open finding.
fn review_findings(issues: &[Issue], branch: Option<&str>) -> Vec<String> {
    let findings: Vec<&Issue> = issues
        .iter()
        .filter(|issue| issue.title.starts_with(FINDING_TITLE))
        .collect();
    let (mine, others): (Vec<&Issue>, Vec<&Issue>) = findings
        .into_iter()
        .partition(|issue| branch.is_some() && issue.branch() == branch);
    let mut lines: Vec<String> = mine
        .iter()
        .map(|issue| {
            format!(
                "Claude review finding #{} of this branch; fix it before other work: {}: {}",
                issue.number,
                issue.title.trim_start_matches(FINDING_TITLE),
                issue.url
            )
        })
        .collect();
    if !others.is_empty() {
        let numbers: Vec<String> = others
            .iter()
            .map(|issue| format!("#{}", issue.number))
            .collect();
        lines.push(format!(
            "{} open Claude review finding(s) of other branches: {}",
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
        "number,title,url,body",
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
    Ok(review_findings(&issues, branch.as_deref()))
}

/// Prints [`report`]'s lines, or one warning line when CI's state is
/// unknown, then the open Claude review findings, prefixed with `label`.
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
    fn findings_of_this_branch_are_listed_and_others_counted() {
        let issues: Vec<Issue> = serde_json::from_str(
            r#"[
            {"number":1,"title":"review: Lost error","url":"u1",
             "body":"Found.\n\n<!-- claude-review-finding: k -->\n<!-- claude-review-branch: fix/a -->"},
            {"number":2,"title":"review: Stale doc","url":"u2",
             "body":"<!-- claude-review-branch: fix/b -->"},
            {"number":3,"title":"review: Filed by hand","url":"u3","body":"No branch."},
            {"number":4,"title":"Unrelated reviewer notes","url":"u4",
             "body":"<!-- claude-review-branch: fix/a -->"}
        ]"#,
        )
        .unwrap();
        assert_eq!(
            review_findings(&issues, Some("fix/a")),
            [
                "Claude review finding #1 of this branch; fix it before other work: Lost error: u1",
                "2 open Claude review finding(s) of other branches: #2, #3",
            ]
        );
        assert_eq!(
            review_findings(&issues, None),
            ["3 open Claude review finding(s) of other branches: #1, #2, #3"]
        );
        assert!(review_findings(&[], Some("fix/a")).is_empty());
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
