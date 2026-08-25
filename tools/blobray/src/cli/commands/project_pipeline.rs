//! CLI adapter for project-owned analysis and review orchestration.

use super::Result;
use crate::application::{ProjectSession, project_analysis::ProjectAnalysisRequest};
use crate::cli::ProjectAnalyzeArgs;

mod plan;
pub(crate) mod status;

pub(super) fn human_duration(duration_ms: Option<u64>) -> String {
    match duration_ms {
        Some(0) => "<1 ms".to_owned(),
        Some(duration_ms) => format!("{duration_ms} ms"),
        None => "not measured".to_owned(),
    }
}

pub(super) fn run(arguments: ProjectAnalyzeArgs, session: &ProjectSession) -> Result<bool> {
    let request = ProjectAnalysisRequest {
        check: arguments.check,
        deny_unreviewed: arguments.deny_unreviewed,
        jobs: usize::from(arguments.jobs),
    }
    .validate()?;
    if arguments.plan {
        let report = crate::application::project_analysis::plan_project(session, request);
        plan::render(&report);
        return Ok(report.succeeded());
    }
    let report = crate::application::project_analysis::analyze_project(session, request);
    status::render(&report);
    Ok(report.succeeded())
}
