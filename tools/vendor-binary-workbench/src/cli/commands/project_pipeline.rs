//! CLI adapter for project-owned analysis and review orchestration.

use super::{MmioMap, Result, TargetSpec};
use crate::cli::ProjectAnalyzeArgs;
use crate::{
    MemoryMap, application::project_analysis::ProjectAnalysisRequest, project::ProjectSpec,
    run_spec::RunSpec,
};

pub(crate) mod status;

pub(super) fn run(
    arguments: ProjectAnalyzeArgs,
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    memory_map: Option<&MemoryMap>,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    if arguments.jobs > 8 {
        return Err(crate::Error::invalid(
            "project analyze --jobs accepts 0 (safe automatic mode) or 1..=8",
        ));
    }
    let request = ProjectAnalysisRequest {
        check: arguments.check,
        deny_unreviewed: arguments.deny_unreviewed,
        jobs: usize::from(arguments.jobs),
    };
    let report = crate::application::project_analysis::analyze_project(
        project, request, run_spec, memory_map, svd, target,
    );
    status::render(&report);
    Ok(report.succeeded())
}
