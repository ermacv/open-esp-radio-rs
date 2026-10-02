//! The state of GitHub CI on `main`, as `check changed` and `push` report it.
//!
//! CI verifies `main` after every push: the source checks, both final HIL
//! images with their audits, and every image class once a night. A failure
//! there does not block a push; it is printed on every `check changed` and
//! `push` until a later run of the same workflow passes, so whoever pushed the
//! failing commit, or anyone who sees it first, fixes it before other work.

use crate::Context;
use oer_process as process;
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

/// For each workflow, its newest run in `runs` that judged its commit, when
/// that run failed. Cancelled and skipped runs are passed over.
pub fn failures(runs: &[Run]) -> Vec<&Run> {
    let mut judged = runs
        .iter()
        .filter(|run| {
            run.status == RunStatus::Completed && run.conclusion.is_some_and(Conclusion::is_verdict)
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
pub fn report(ctx: &Context) -> Result<Vec<String>, String> {
    report_with(ctx, "gh")
}

fn report_with(ctx: &Context, gh: &str) -> Result<Vec<String>, String> {
    let output = process::capture(ctx.command(gh).args([
        "run",
        "list",
        "--branch",
        "main",
        "--limit",
        "30",
        "--json",
        "workflowName,headSha,status,conclusion,url,databaseId,createdAt",
    ]))
    .map_err(|error| match error.downcast_ref::<std::io::Error>() {
        Some(io) if io.kind() == std::io::ErrorKind::NotFound => "`gh` is not installed".to_owned(),
        _ => error
            .to_string()
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("`gh run list` failed")
            .trim()
            .to_owned(),
    })?;
    let runs = serde_json::from_slice::<Vec<Run>>(&output.stdout)
        .map_err(|error| format!("`gh run list` printed no run list: {error}"))?;
    Ok(failures(&runs)
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

/// Prints [`report`]'s lines, or one warning line when CI's state is
/// unknown, prefixed with `label`.
pub fn print(ctx: &Context, label: &str) {
    match report(ctx) {
        Ok(failures) => {
            for failure in failures {
                println!("{label}: {failure}");
            }
        }
        Err(reason) => println!("{label}: warning: the state of CI on main is unknown: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(json: &str) -> Vec<Run> {
        serde_json::from_str(json).unwrap()
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
        let failed = failures(&runs);
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
        let failed = failures(&runs);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].head_sha, "mid");
    }

    #[test]
    fn a_missing_gh_is_reported_not_taken_for_green() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        let ctx = Context::new(directory.path()).unwrap();
        assert_eq!(
            report_with(&ctx, "oer-gh-that-does-not-exist"),
            Err("`gh` is not installed".to_owned())
        );
    }

    #[test]
    fn an_unknown_status_or_conclusion_still_parses() {
        let runs = runs(
            r#"[{"workflowName":"CI","headSha":"c","status":"brand_new","conclusion":"surprise","url":"u","databaseId":1,"createdAt":"2026-09-29T01:00:00Z"}]"#,
        );
        assert_eq!(runs[0].status, RunStatus::Other);
        assert_eq!(runs[0].conclusion, Some(Conclusion::Other));
        assert!(failures(&runs).is_empty());
    }
}
