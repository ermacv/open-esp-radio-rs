//! CLI adapter for project-owned analysis and review orchestration.

use super::Result;
use crate::application::{ProjectSession, project_analysis::ProjectAnalysisRequest};
use crate::cli::ProjectAnalyzeArgs;

pub(crate) mod status;

pub(super) fn run(arguments: ProjectAnalyzeArgs, session: &ProjectSession) -> Result<bool> {
    if !(1..=8).contains(&arguments.jobs) {
        return Err(crate::Error::invalid(
            "project analyze --jobs accepts 1..=8",
        ));
    }
    let request = ProjectAnalysisRequest {
        check: arguments.check,
        deny_unreviewed: arguments.deny_unreviewed,
        jobs: usize::from(arguments.jobs),
    };
    let report = crate::application::project_analysis::analyze_project(session, request);
    status::render(&report);
    Ok(report.succeeded())
}
