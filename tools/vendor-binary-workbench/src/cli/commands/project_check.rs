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
    let binding_audit = crate::verification::audit(&session.project)?;
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
    let policy = policy_stage(session);
    let ownership = crate::application::project_files::collect_ownership(&session.project)?;

    let passed = ownership.passed()
        && binding_audit.passed
        && analysis.succeeded()
        && verification.passed
        && policy.passed
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
        schema: 6,
        command: "project check",
        project: session.project.id.clone(),
        passed,
        stages: vec![
            ProjectCheckStage {
                name: "file-ownership",
                status: if ownership.passed() { "passed" } else { "failed" },
                passed: ownership.passed(),
                summary: format!(
                    "{} unowned reviewed asset(s), {} stale generated report(s)",
                    ownership.unowned_reviewed.len(),
                    ownership.stale_generated.len(),
                ),
                issues: ownership
                    .unowned_reviewed
                    .iter()
                    .map(|path| ProjectCheckIssue {
                        component: "unowned-reviewed-asset".to_owned(),
                        status: "failed".to_owned(),
                        reason: path.display().to_string(),
                    })
                    .chain(ownership.stale_generated.iter().map(|path| {
                        ProjectCheckIssue {
                            component: "stale-generated-report".to_owned(),
                            status: "failed".to_owned(),
                            reason: path.display().to_string(),
                        }
                    }))
                    .collect(),
                next_actions: (!ownership.passed())
                    .then(|| {
                        format!(
                            "remove the {} files without a current project owner, then rerun `vendor-binary-workbench project check --project {}`",
                            ownership.issue_count(),
                            session.manifest.display()
                        )
                    })
                    .into_iter()
                    .collect(),
            },
            ProjectCheckStage {
                name: "binding-declarations",
                status: if binding_audit.passed { "passed" } else { "failed" },
                passed: binding_audit.passed,
                summary: format!(
                    "{} valid declarations, {} required by policy, {} invalid; execution is evaluated by the verification stage",
                    binding_audit.declared,
                    binding_audit.verification_required,
                    binding_audit.invalid,
                ),
                issues: binding_audit
                    .bindings
                    .iter()
                    .filter(|binding| binding.status == "invalid")
                    .map(|binding| ProjectCheckIssue {
                        component: format!("{}:{}", binding.source, binding.vendor_symbol),
                        status: binding.status.to_owned(),
                        reason: binding
                            .blocker
                            .clone()
                            .unwrap_or_else(|| "binding declaration is invalid".to_owned()),
                    })
                    .collect(),
                next_actions: (!binding_audit.passed)
                    .then(|| {
                        format!(
                            "run `vendor-binary-workbench project audit bindings --project {}`; fix every invalid declaration and every verification-blocked binding required by the verification policy before accepting baselines",
                            session.manifest.display()
                        )
                    })
                    .into_iter()
                    .collect(),
            },
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
                    "{} suites, {} whole-function matches, {} bounded matches, {} mismatches, {} incomplete, {} implemented without executable proof",
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
            policy,
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
        output::success("PASS — analysis, verification policy and publication reproduce")
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

fn policy_stage(session: &ProjectSession) -> ProjectCheckStage {
    let manifest = session.manifest.display();
    let Some(policy_path) = session
        .project
        .verification
        .as_ref()
        .and_then(|verification| verification.policy.as_ref())
    else {
        return ProjectCheckStage {
            name: "verification-policy",
            status: "failed",
            passed: false,
            summary: "verification policy is not configured".to_owned(),
            issues: vec![ProjectCheckIssue {
                component: "verification-policy".to_owned(),
                status: "failed".to_owned(),
                reason: "every project check requires an explicit flat verification policy"
                    .to_owned(),
            }],
            next_actions: vec![format!("configure verification.policy in {manifest}")],
        };
    };
    match crate::verification::policy::evaluate(&session.project) {
        Ok(Some(report)) => {
            let issues = report
                .surfaces
                .iter()
                .filter(|surface| !surface.closed)
                .flat_map(|surface| {
                    surface.blockers.iter().map(|blocker| ProjectCheckIssue {
                        component: format!("surface:{}", surface.id),
                        status: "failed".to_owned(),
                        reason: blocker.clone(),
                    })
                })
                .collect();
            ProjectCheckStage {
                name: "verification-policy",
                status: if report.passed { "passed" } else { "failed" },
                passed: report.passed,
                summary: format!(
                    "{} closed verification surface(s), {} blocked",
                    report.closed, report.blocked,
                ),
                issues,
                next_actions: (!report.passed)
                    .then(|| {
                        format!(
                            "close the reported verification surface and rerun `vendor-binary-workbench project check --project {manifest}`"
                        )
                    })
                    .into_iter()
                    .collect(),
            }
        }
        Ok(None) => unreachable!("policy path was present"),
        Err(error) => ProjectCheckStage {
            name: "verification-policy",
            status: "failed",
            passed: false,
            summary: format!(
                "cannot evaluate verification policy from {}",
                policy_path.display()
            ),
            issues: vec![ProjectCheckIssue {
                component: "verification-policy".to_owned(),
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
    if report.passed {
        // Informational suites deliberately retain DIFF/INCOMPLETE facts in
        // the replacement summary. They belong in the verification-stage
        // summary, not in the list of failed project gates.
        return Vec::new();
    }
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
            schema: 6,
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
                    reason: "1: missing production trace".to_owned(),
                }],
                next_actions: vec!["inspect verification.json".to_owned()],
            }],
        };
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["schema"], 6);
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
