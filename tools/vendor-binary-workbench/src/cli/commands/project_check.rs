//! One authoritative, non-mutating project CI workflow.

use serde::Serialize;

use super::Result;
use crate::{
    application::{
        ProjectSession,
        pipeline::StageReport,
        project_analysis::ProjectAnalysisRequest,
        project_publication::{ProjectPublicationRequest, execute as publish},
    },
    cli::{ProjectCheckArgs, ProjectVerifyArgs},
};

#[derive(Serialize)]
struct ProjectCheckReport {
    schema: u32,
    command: &'static str,
    project: String,
    passed: bool,
    stages: Vec<ProjectCheckStage>,
}

#[derive(Serialize)]
struct ProjectCheckStage {
    name: &'static str,
    status: &'static str,
    passed: bool,
    summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    issues: Vec<ProjectCheckIssue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    next_actions: Vec<String>,
}

#[derive(Serialize)]
struct ProjectCheckIssue {
    component: String,
    status: String,
    reason: String,
}

pub(super) fn run(arguments: ProjectCheckArgs, session: &ProjectSession) -> Result<bool> {
    if arguments.jobs > 8 {
        return Err(crate::Error::invalid(
            "project check --jobs accepts 0 (safe automatic mode) or 1..=8",
        ));
    }
    let analysis = crate::application::project_analysis::analyze_project(
        session,
        ProjectAnalysisRequest {
            check: true,
            deny_unreviewed: arguments.deny_unreviewed,
            jobs: usize::from(arguments.jobs),
        },
    );
    let verification = super::project_verification::execute(
        ProjectVerifyArgs {
            check: true,
            ..ProjectVerifyArgs::default()
        },
        &session.manifest,
        &session.project,
        session.run_spec.as_ref(),
        &session.mmio,
        &session.target,
    )?;
    let publication = publish(
        &session.project,
        session.memory_map.as_ref(),
        ProjectPublicationRequest { check: true },
    )?;

    let passed = analysis.succeeded() && verification.passed && publication.succeeded();
    let analysis_passed = analysis.succeeded();
    let publication_passed = publication.succeeded();
    let report_path = &session
        .project
        .verification
        .as_ref()
        .expect("verification was executed")
        .report;
    let graph = &verification.replacement_graph.summary;
    let report = ProjectCheckReport {
        schema: 2,
        command: "project check",
        project: session.project.id.clone(),
        passed,
        stages: vec![
            ProjectCheckStage {
                name: "analysis",
                status: if analysis_passed { "passed" } else { "failed" },
                passed: analysis_passed,
                summary: format!(
                    "{} verified/current, {} failed, {} blocked",
                    analysis.verified + analysis.current,
                    analysis.failed,
                    analysis.blocked
                ),
                issues: pipeline_issues(&analysis.stages),
                next_actions: (!analysis_passed)
                    .then(|| {
                        format!(
                            "inspect failed analysis stages above and rerun `vendor-binary-workbench project analyze --check --project {}`",
                            session.manifest.display()
                        )
                    })
                    .into_iter()
                    .collect(),
            },
            ProjectCheckStage {
                name: "verification",
                status: if verification.passed { "passed" } else { "failed" },
                passed: verification.passed,
                summary: format!(
                    "{} suites, {} matches, {} mismatches, {} incomplete, {} implemented-unqualified",
                    verification.suites.len(),
                    graph.behavioral_matches,
                    graph.mismatches,
                    graph.incomplete,
                    graph.implemented_unqualified,
                ),
                issues: verification_issues(&verification),
                next_actions: (!verification.passed)
                    .then(|| {
                        format!(
                            "inspect {}; edit the responsible verification dispositions/contracts and rerun `vendor-binary-workbench project verify --check --project {}`",
                            report_path.display(),
                            session.manifest.display()
                        )
                    })
                    .into_iter()
                    .collect(),
            },
            ProjectCheckStage {
                name: "publication",
                status: if publication_passed { "passed" } else { "failed" },
                passed: publication_passed,
                summary: format!(
                    "{} verified, {} failed, {} blocked",
                    publication.verified, publication.failed, publication.blocked
                ),
                issues: pipeline_issues(&publication.stages),
                next_actions: (!publication_passed)
                    .then(|| {
                        format!(
                            "fix the publication issue above and rerun `vendor-binary-workbench project publish --check --project {}`",
                            session.manifest.display()
                        )
                    })
                    .into_iter()
                    .collect(),
            },
        ],
    };
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(passed)
}

fn render_human(report: &ProjectCheckReport) {
    outputln!(
        "Project check: {} — {}",
        if report.passed { "passed" } else { "failed" },
        report.project
    );
    for stage in &report.stages {
        outputln!("  {:<14} {:<8} {}", stage.name, stage.status, stage.summary);
        for issue in &stage.issues {
            outputln!(
                "    issue: {} [{}] — {}",
                issue.component,
                issue.status,
                issue.reason
            );
        }
        for action in &stage.next_actions {
            outputln!("    next: {action}");
        }
    }
}

fn pipeline_issues(stages: &[StageReport]) -> Vec<ProjectCheckIssue> {
    stages
        .iter()
        .filter(|stage| matches!(stage.status, "failed" | "blocked"))
        .map(|stage| ProjectCheckIssue {
            component: stage.name.clone(),
            status: stage.status.to_owned(),
            reason: stage
                .reason
                .clone()
                .unwrap_or_else(|| "stage did not complete".to_owned()),
        })
        .collect()
}

fn verification_issues(
    report: &crate::verification::ProjectVerificationReport,
) -> Vec<ProjectCheckIssue> {
    let summary = &report.replacement_graph.summary;
    let mut issues = Vec::new();
    for (component, count, reason) in [
        (
            "replacement-mismatches",
            summary.mismatches,
            "vendor and Rust effects differ",
        ),
        (
            "replacement-incomplete",
            summary.incomplete,
            "one or more comparisons lack complete evidence",
        ),
        (
            "implemented-unqualified",
            summary.implemented_unqualified,
            "implemented replacements have no completed executable qualification",
        ),
    ] {
        if count != 0 {
            issues.push(ProjectCheckIssue {
                component: component.to_owned(),
                status: "failed".to_owned(),
                reason: format!("{count}: {reason}"),
            });
        }
    }
    if !report.passed && issues.is_empty() {
        for suite in report
            .suites
            .iter()
            .filter(|suite| !suite.verification.verification.passed)
        {
            issues.push(ProjectCheckIssue {
                component: format!("suite:{}", suite.id),
                status: "failed".to_owned(),
                reason: "verification suite gate did not pass".to_owned(),
            });
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_failures_remain_typed_and_actionable() {
        let issues = pipeline_issues(&[
            StageReport {
                name: "linked-ir".to_owned(),
                status: "failed",
                reason: Some("stale output /tmp/linked.ir.json".to_owned()),
            },
            StageReport {
                name: "function-review".to_owned(),
                status: "blocked",
                reason: Some("linked-ir did not complete".to_owned()),
            },
            StageReport {
                name: "register-validation".to_owned(),
                status: "verified",
                reason: None,
            },
        ]);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].component, "linked-ir");
        assert!(issues[0].reason.contains("/tmp/linked.ir.json"));
        assert_eq!(issues[1].status, "blocked");
    }

    #[test]
    fn aggregate_schema_keeps_issues_and_next_actions_separate() {
        let report = ProjectCheckReport {
            schema: 2,
            command: "project check",
            project: "fixture".to_owned(),
            passed: false,
            stages: vec![ProjectCheckStage {
                name: "verification",
                status: "failed",
                passed: false,
                summary: "one unqualified replacement".to_owned(),
                issues: vec![ProjectCheckIssue {
                    component: "implemented-unqualified".to_owned(),
                    status: "failed".to_owned(),
                    reason: "1: missing qualification".to_owned(),
                }],
                next_actions: vec!["inspect verification.json".to_owned()],
            }],
        };
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["schema"], 2);
        assert_eq!(value["stages"][0]["status"], "failed");
        assert_eq!(
            value["stages"][0]["issues"][0]["component"],
            "implemented-unqualified"
        );
        assert_eq!(
            value["stages"][0]["next_actions"][0],
            "inspect verification.json"
        );
    }
}
