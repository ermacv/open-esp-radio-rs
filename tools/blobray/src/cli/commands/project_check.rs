//! One authoritative, non-mutating project CI workflow.

use serde::Serialize;

use super::Result;
use crate::{
    application::{
        ExecutableAction, FollowUpStep, ProjectContext, ProjectContextRequirement, ProjectSession,
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
    next_steps: Vec<FollowUpStep>,
}

#[derive(Serialize)]
struct ProjectCheckIssue {
    component: String,
    status: String,
    reason: String,
}

struct ProjectCheckFollowUps {
    check: ExecutableAction,
    audit_bindings: ExecutableAction,
    analyze_check: ExecutableAction,
    verify_check: ExecutableAction,
    publish_check: ExecutableAction,
}

impl ProjectCheckFollowUps {
    fn new(context: &ProjectContext<'_>) -> Result<Self> {
        Ok(Self {
            check: context
                .follow_up_action(["project", "check"], ProjectContextRequirement::Analysis)?,
            audit_bindings: context.follow_up_action(
                ["project", "audit", "bindings"],
                ProjectContextRequirement::Target,
            )?,
            analyze_check: context.follow_up_action(
                ["project", "analyze", "--check"],
                ProjectContextRequirement::Analysis,
            )?,
            verify_check: context.follow_up_action(
                ["project", "verify", "--check"],
                ProjectContextRequirement::Analysis,
            )?,
            publish_check: context.follow_up_action(
                ["project", "publish", "--check"],
                ProjectContextRequirement::ProjectOnly,
            )?,
        })
    }
}

pub(super) fn run(arguments: ProjectCheckArgs, session: &ProjectSession) -> Result<bool> {
    let context = session.context();
    let follow_ups = ProjectCheckFollowUps::new(&context)?;
    let binding_audit = crate::verification::audit(&session.project)?;
    let analysis_request = ProjectAnalysisRequest {
        check: true,
        deny_unreviewed: arguments.deny_unreviewed,
        jobs: usize::from(arguments.jobs),
    }
    .validate()?;
    let analysis = crate::application::project_analysis::analyze_project(session, analysis_request);
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
    let policy = policy_stage(session, &follow_ups.check);
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
        schema: 7,
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
                next_steps: (!ownership.passed())
                    .then(|| {
                        FollowUpStep::command(
                            format!(
                                "Remove the {} files without a current project owner, then rerun the project check.",
                                ownership.issue_count(),
                            ),
                            follow_ups.check.clone(),
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
                next_steps: (!binding_audit.passed)
                    .then(|| {
                        FollowUpStep::command(
                            "Inspect the binding audit, then fix every invalid declaration and every policy-required verification-blocked binding before accepting baselines.",
                            follow_ups.audit_bindings.clone(),
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
                next_steps: (!analysis_passed)
                    .then(|| {
                        FollowUpStep::command(
                            "Inspect the failed analysis stages above and reproduce the analysis check.",
                            follow_ups.analyze_check.clone(),
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
                next_steps: (!verification.passed)
                    .then(|| {
                        FollowUpStep::command(
                            format!(
                                "Inspect {}; edit the responsible verification dispositions or contracts, then reproduce verification.",
                                report_path.display(),
                            ),
                            follow_ups.verify_check.clone(),
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
                next_steps: (!publication_passed)
                    .then(|| {
                        FollowUpStep::command(
                            "Fix the publication issue above, then reproduce the publication preflight.",
                            follow_ups.publish_check.clone(),
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

    let steps = report
        .stages
        .iter()
        .flat_map(|stage| &stage.next_steps)
        .collect::<Vec<_>>();
    if !steps.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, step) in steps.iter().enumerate() {
            outputln!("{}. {}", index + 1, step.instruction);
            for command in &step.commands {
                outputln!("   {}", command.render_posix());
            }
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

fn policy_stage(session: &ProjectSession, check_command: &ExecutableAction) -> ProjectCheckStage {
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
            next_steps: vec![FollowUpStep::manual(format!(
                "Configure verification.policy in {manifest}."
            ))],
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
                next_steps: (!report.passed)
                    .then(|| {
                        FollowUpStep::command(
                            "Close the reported verification surface, then rerun the project check.",
                            check_command.clone(),
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
            next_steps: vec![FollowUpStep::command(
                "Regenerate the analysis and verification reports, then rerun the project check.",
                check_command.clone(),
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
    use crate::{MmioMap, TargetSpec, application::ExplicitProjectContext, project::ProjectSpec};
    use std::path::{Path, PathBuf};

    #[test]
    fn pipeline_failures_remain_typed_and_actionable() {
        let issues = pipeline_issues(&[
            StageReport {
                name: "linked-ir".to_owned(),
                status: "failed",
                duration_ms: Some(17),
                reason: Some("stale output /tmp/linked.ir".to_owned()),
            },
            StageReport {
                name: "function-review".to_owned(),
                status: "blocked",
                duration_ms: None,
                reason: Some("linked-ir did not complete".to_owned()),
            },
            StageReport {
                name: "register-validation".to_owned(),
                status: "verified",
                duration_ms: Some(0),
                reason: None,
            },
        ]);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].component, "linked-ir");
        assert!(issues[0].reason.contains("/tmp/linked.ir"));
        assert_eq!(issues[1].status, "blocked");
    }

    #[test]
    fn aggregate_schema_keeps_issues_and_next_steps_separate() {
        let report = ProjectCheckReport {
            schema: 7,
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
                next_steps: vec![FollowUpStep::manual("Inspect verification.json.")],
            }],
        };
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["schema"], 7);
        assert_eq!(value["stages"][0]["status"], "failed");
        assert_eq!(
            value["stages"][0]["issues"][0]["component"],
            "implemented-unqualified"
        );
        assert_eq!(
            value["stages"][0]["next_steps"][0]["instruction"],
            "Inspect verification.json."
        );
        assert!(
            value["stages"][0]["next_steps"][0]["commands"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(value["stages"][0].get("next_actions").is_none());
    }

    #[test]
    fn check_follow_ups_quote_context_and_scope_overrides_per_destination() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generic-project/vendor-project.toml");
        let project = ProjectSpec::load(&fixture).unwrap();
        let target = TargetSpec::load(&project.target_spec).unwrap();
        let svd = MmioMap::load_all(&[]).unwrap();
        let manifest = PathBuf::from("/tmp/vendor  owner's project/vendor project.toml");
        let explicit_target = PathBuf::from("/tmp/target owner's.toml");
        let explicit_run = PathBuf::from("/tmp/run spec.toml");
        let explicit_svds = vec![
            PathBuf::from("/tmp/registers one.svd"),
            PathBuf::from("/tmp/register owner's two.svd"),
        ];
        let explicit_context = ExplicitProjectContext {
            target_spec: Some(explicit_target.clone()),
            run_spec: Some(explicit_run.clone()),
            svd_paths: explicit_svds.clone(),
        };
        let context = ProjectContext {
            project_path: &manifest,
            project: &project,
            target_path: &project.target_spec,
            target: &target,
            run_spec_path: None,
            run_spec: None,
            memory_map: None,
            svd_paths: &[],
            svd: &svd,
            explicit_context: &explicit_context,
            invocation_directory: Path::new("/tmp"),
        };
        let follow_ups = ProjectCheckFollowUps::new(&context).unwrap();
        let analysis_context = [
            "--project",
            manifest.to_str().unwrap(),
            "--target-spec",
            explicit_target.to_str().unwrap(),
            "--run-spec",
            explicit_run.to_str().unwrap(),
            "--svd",
            explicit_svds[0].to_str().unwrap(),
            "--svd",
            explicit_svds[1].to_str().unwrap(),
        ];

        assert_eq!(
            follow_ups.check.argv,
            ["blobray", "project", "check"]
                .into_iter()
                .chain(analysis_context)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            follow_ups.analyze_check.argv,
            ["blobray", "project", "analyze", "--check"]
                .into_iter()
                .chain(analysis_context)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            follow_ups.verify_check.argv,
            ["blobray", "project", "verify", "--check"]
                .into_iter()
                .chain(analysis_context)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            follow_ups.audit_bindings.argv,
            [
                "blobray",
                "project",
                "audit",
                "bindings",
                "--project",
                manifest.to_str().unwrap(),
                "--target-spec",
                explicit_target.to_str().unwrap(),
            ]
        );
        assert_eq!(
            follow_ups.publish_check.argv,
            [
                "blobray",
                "project",
                "publish",
                "--check",
                "--project",
                manifest.to_str().unwrap(),
            ]
        );
    }
}
