//! CLI adapter for project-owned analysis and review orchestration.

use super::{MmioMap, Result, TargetSpec};
use crate::cli::ProjectAnalyzeArgs;
use crate::{
    MemoryMap,
    application::project_analysis::{
        ProjectAnalysisInputs, ProjectAnalysisOperations, ProjectAnalysisRequest,
    },
    project::ProjectSpec,
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
    let request = ProjectAnalysisRequest {
        check: arguments.check,
        deny_unreviewed: arguments.deny_unreviewed,
    };
    let inputs = ProjectAnalysisInputs {
        run_spec: run_spec.is_some(),
        memory_map: memory_map.is_some(),
    };
    let mut operations = CliProjectAnalysisOperations {
        project,
        run_spec,
        memory_map,
        svd,
        target,
    };
    let report = crate::cli::output::suppress(|| {
        crate::application::project_analysis::run(project, request, inputs, &mut operations)
    });
    status::render(&report);
    Ok(report.succeeded())
}

struct CliProjectAnalysisOperations<'a> {
    project: &'a ProjectSpec,
    run_spec: Option<&'a RunSpec>,
    memory_map: Option<&'a MemoryMap>,
    svd: &'a MmioMap,
    target: &'a TargetSpec,
}

impl CliProjectAnalysisOperations<'_> {
    fn run_spec(&self) -> Result<&RunSpec> {
        self.run_spec
            .ok_or_else(|| crate::Error::invalid("run-spec is not configured"))
    }

    fn memory_map(&self) -> Result<&MemoryMap> {
        self.memory_map
            .ok_or_else(|| crate::Error::invalid("memory-map is not configured"))
    }
}

impl ProjectAnalysisOperations for CliProjectAnalysisOperations<'_> {
    fn symbol_inventory(&mut self, check: bool) -> Result<bool> {
        crate::application::project_analysis::build_symbol_inventory(
            self.project,
            self.run_spec()?,
            check,
        )
    }

    fn discover_mmio(&mut self, check: bool) -> Result<bool> {
        crate::application::project_analysis::discover_project_mmio(
            self.project,
            self.run_spec()?,
            self.memory_map()?,
            self.svd,
            check,
        )
    }

    fn discover_interfaces(&mut self, check: bool) -> Result<bool> {
        crate::application::project_analysis::discover_project_interfaces_operation(
            self.project,
            self.run_spec()?,
            check,
        )
    }

    fn build_linked_ir(&mut self, check: bool) -> Result<bool> {
        crate::application::project_ir_build::build_project_ir(
            crate::application::project_ir_build::ProjectIrBuildRequest {
                profiles: Default::default(),
                check,
            },
            self.project,
            self.run_spec()?,
            self.svd,
            self.target,
        )?;
        Ok(true)
    }

    fn build_navigation(&mut self, check: bool) -> Result<bool> {
        crate::application::project_analysis::build_navigation(self.project, check)
    }

    fn validate_registers(&mut self, deny_unreviewed: bool) -> Result<bool> {
        crate::application::project_analysis::validate_registers(
            self.project,
            self.memory_map,
            deny_unreviewed,
        )
    }

    fn review_registers(&mut self, check: bool) -> Result<bool> {
        crate::application::project_analysis::review_registers(self.project, check)
    }

    fn validate_functions(&mut self, deny_unreviewed: bool) -> Result<bool> {
        crate::application::project_analysis::validate_functions(self.project, deny_unreviewed)
    }

    fn review_functions(&mut self, check: bool) -> Result<bool> {
        crate::application::project_analysis::review_functions(self.project, self.target, check)
    }

    fn validate_interfaces(&mut self, deny_unreviewed: bool) -> Result<bool> {
        crate::application::project_analysis::validate_interfaces(
            self.project,
            self.target,
            deny_unreviewed,
        )
    }
}
