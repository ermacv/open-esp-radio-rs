//! One authoritative, non-mutating project CI workflow.

use serde::Serialize;

use super::Result;
use crate::{
    application::{
        ProjectSession,
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
    passed: bool,
    summary: String,
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
    let report = ProjectCheckReport {
        schema: 1,
        command: "project check",
        project: session.project.id.clone(),
        passed,
        stages: vec![
            ProjectCheckStage {
                name: "analysis",
                passed: analysis.succeeded(),
                summary: format!(
                    "{} verified/current, {} failed, {} blocked",
                    analysis.verified + analysis.current,
                    analysis.failed,
                    analysis.blocked
                ),
            },
            ProjectCheckStage {
                name: "verification",
                passed: verification.passed,
                summary: format!("{} suites", verification.suites.len()),
            },
            ProjectCheckStage {
                name: "publication",
                passed: publication.succeeded(),
                summary: format!(
                    "{} verified, {} failed, {} blocked",
                    publication.verified, publication.failed, publication.blocked
                ),
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
        outputln!(
            "  {:<14} {:<8} {}",
            stage.name,
            if stage.passed { "passed" } else { "failed" },
            stage.summary
        );
    }
}
