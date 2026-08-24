//! One authoritative, non-mutating project CI workflow.

use serde::Serialize;

use super::Result;
use crate::{
    application::{
        FollowUpRequirements, ProjectContext, ProjectSession,
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

struct ProjectCheckFollowUps {
    check: String,
    audit_bindings: String,
    analyze_check: String,
    verify_check: String,
    publish_check: String,
}

impl ProjectCheckFollowUps {
    fn new(context: &ProjectContext<'_>) -> Self {
        Self {
            check: context.follow_up_command("project check", FollowUpRequirements::ANALYSIS),
            audit_bindings: context
                .follow_up_command("project audit bindings", FollowUpRequirements::TARGET),
            analyze_check: context
                .follow_up_command("project analyze --check", FollowUpRequirements::ANALYSIS),
            verify_check: context
                .follow_up_command("project verify --check", FollowUpRequirements::ANALYSIS),
            publish_check: context.follow_up_command(
                "project publish --check",
                FollowUpRequirements::PROJECT_ONLY,
            ),
        }
    }
}

pub(super) fn run(arguments: ProjectCheckArgs, session: &ProjectSession) -> Result<bool> {
    let context = session.context();
    let follow_ups = ProjectCheckFollowUps::new(&context);
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
                            "remove the {} files without a current project owner, then rerun `{}`",
                            ownership.issue_count(),
                            follow_ups.check,
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
                            "run `{}`; fix every invalid declaration and every verification-blocked binding required by the verification policy before accepting baselines",
                            follow_ups.audit_bindings,
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
                            "inspect failed analysis stages above and rerun `{}`",
                            follow_ups.analyze_check,
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
                            "inspect {}; edit the responsible verification dispositions/contracts and rerun `{}`",
                            report_path.display(),
                            follow_ups.verify_check,
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
                            "fix the publication issue above and rerun `{}`",
                            follow_ups.publish_check,
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

fn policy_stage(session: &ProjectSession, check_command: &str) -> ProjectCheckStage {
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
                            "close the reported verification surface and rerun `{check_command}`"
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
                "regenerate analysis and verification reports, then rerun `{check_command}`"
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
        };
        let arg = |path: &Path| crate::shell::arg(path.as_os_str());
        let follow_ups = ProjectCheckFollowUps::new(&context);
        let analysis_context = format!(
            "--project {} --target-spec {} --run-spec {} --svd {} --svd {}",
            arg(&manifest),
            arg(&explicit_target),
            arg(&explicit_run),
            arg(&explicit_svds[0]),
            arg(&explicit_svds[1]),
        );

        assert_eq!(
            follow_ups.check,
            format!("blobray project check {analysis_context}")
        );
        assert_eq!(
            follow_ups.analyze_check,
            format!("blobray project analyze --check {analysis_context}")
        );
        assert_eq!(
            follow_ups.verify_check,
            format!("blobray project verify --check {analysis_context}")
        );
        assert_eq!(
            follow_ups.audit_bindings,
            format!(
                "blobray project audit bindings --project {} --target-spec {}",
                arg(&manifest),
                arg(&explicit_target),
            )
        );
        assert_eq!(
            follow_ups.publish_check,
            format!(
                "blobray project publish --check --project {}",
                arg(&manifest)
            )
        );
    }
}
