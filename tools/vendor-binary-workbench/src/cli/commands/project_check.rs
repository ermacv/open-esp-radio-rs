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
    if !(1..=8).contains(&arguments.jobs) {
        return Err(crate::Error::invalid("project check --jobs accepts 1..=8"));
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
    let qualification = qualification_stage(
        session,
        verification.replacement_graph.summary.bounded_matches,
    );

    let passed = analysis.succeeded()
        && verification.passed
        && qualification.passed
        && publication.succeeded();
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
        schema: 3,
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
                    "{} suites, {} whole-function matches, {} bounded feature matches, {} mismatches, {} incomplete, {} implemented-unqualified",
                    verification.suites.len(),
                    graph.behavioral_matches,
                    graph.bounded_matches,
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
            qualification,
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
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Project check"));
    outputln!("Project: {}", report.project);
    let outcome = if report.passed {
        output::success(
            "PASS — analysis, verification, feature qualification and publication reproduce",
        )
    } else {
        output::failure("FAIL — one or more project gates did not reproduce")
    };
    outputln!("\n{outcome}");

    let issues = report
        .stages
        .iter()
        .flat_map(|stage| &stage.issues)
        .collect::<Vec<_>>();
    if !issues.is_empty() {
        outputln!("\n{}", output::heading("Problems"));
        for (index, issue) in issues.iter().enumerate() {
            outputln!(
                "{}. {} [{}]: {}",
                index + 1,
                issue.component,
                issue.status,
                issue.reason
            );
        }
    }

    let actions = report
        .stages
        .iter()
        .flat_map(|stage| &stage.next_actions)
        .collect::<Vec<_>>();
    if !actions.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, action) in actions.iter().enumerate() {
            outputln!("{}. {action}", index + 1);
        }
    }

    outputln!("\n{}", output::heading("Gates"));
    outputln!(
        "{}",
        table::render(
            ["Gate", "Status", "Summary"],
            report.stages.iter().map(|stage| [
                stage.name.to_owned(),
                stage.status.to_owned(),
                stage.summary.clone(),
            ])
        )
    );
}

fn qualification_stage(session: &ProjectSession, bounded_matches: usize) -> ProjectCheckStage {
    let manifest = session.manifest.display();
    let Some(workspace) = session.project.qualification.as_ref() else {
        let passed = bounded_matches == 0;
        return ProjectCheckStage {
            name: "qualification",
            status: if passed { "not-configured" } else { "failed" },
            passed,
            summary: if passed {
                "not configured; no bounded feature evidence is present".to_owned()
            } else {
                format!(
                    "{bounded_matches} bounded feature match(es) have no project qualification gate"
                )
            },
            issues: (!passed)
                .then(|| ProjectCheckIssue {
                    component: "bounded-feature-qualification".to_owned(),
                    status: "failed".to_owned(),
                    reason: "bounded evidence must be selected by an explicit required feature"
                        .to_owned(),
                })
                .into_iter()
                .collect(),
            next_actions: (!passed)
                .then(|| format!("configure [qualification] in {manifest}"))
                .into_iter()
                .collect(),
        };
    };
    match crate::qualification::evaluate(&session.project) {
        Ok(features) => {
            let required = features
                .iter()
                .filter(|feature| feature.required)
                .collect::<Vec<_>>();
            let blocked = required
                .iter()
                .filter(|feature| {
                    feature.status == crate::qualification::FeatureQualificationStatus::Blocked
                })
                .collect::<Vec<_>>();
            let passed = !required.is_empty() && blocked.is_empty();
            let issues = blocked
                .iter()
                .flat_map(|feature| {
                    feature.blockers.iter().map(|blocker| ProjectCheckIssue {
                        component: format!("feature:{}", feature.id),
                        status: "failed".to_owned(),
                        reason: blocker.clone(),
                    })
                })
                .collect();
            ProjectCheckStage {
                name: "qualification",
                status: if passed { "passed" } else { "failed" },
                passed,
                summary: format!(
                    "{} required feature(s), {} qualified, {} blocked, {} bounded match(es)",
                    required.len(),
                    required.len().saturating_sub(blocked.len()),
                    blocked.len(),
                    bounded_matches,
                ),
                issues,
                next_actions: (!passed)
                    .then(|| {
                        format!(
                            "close the reported feature boundary and rerun `vendor-binary-workbench project check --project {manifest}`"
                        )
                    })
                    .into_iter()
                    .collect(),
            }
        }
        Err(error) => ProjectCheckStage {
            name: "qualification",
            status: "failed",
            passed: false,
            summary: format!(
                "cannot evaluate required features from {}",
                workspace.pack.display()
            ),
            issues: vec![ProjectCheckIssue {
                component: "required-features".to_owned(),
                status: "failed".to_owned(),
                reason: error.to_string(),
            }],
            next_actions: vec![format!(
                "regenerate analysis and verification reports, then rerun `vendor-binary-workbench project check --project {manifest}`"
            )],
        },
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
                reason: Some("stale output /tmp/linked.ir".to_owned()),
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
        assert!(issues[0].reason.contains("/tmp/linked.ir"));
        assert_eq!(issues[1].status, "blocked");
    }

    #[test]
    fn aggregate_schema_keeps_issues_and_next_actions_separate() {
        let report = ProjectCheckReport {
            schema: 3,
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
        assert_eq!(value["schema"], 3);
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
