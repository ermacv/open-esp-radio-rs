//! CLI adapter for project-owned analysis and review orchestration.

use std::path::Path;

use super::{MmioMap, Result, TargetSpec};
use crate::cli::{
    InterfaceDiscoverArgs, IrBuildArgs, MmioDiscoverArgs, NamedAddressRange, ProjectAnalyzeArgs,
    SourcePath, SymbolInventoryArgs,
};
use crate::{
    MemoryMap,
    application::project_analysis::{
        ProjectAnalysisInputs, ProjectAnalysisOperations, ProjectAnalysisRequest,
    },
    project::ProjectSpec,
    run_spec::{InputRole, RunSpec},
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
        let output = self
            .project
            .symbol_inventory
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("[analysis.symbols] is absent"))?
            .output
            .clone();
        super::symbol_inventory::run(
            SymbolInventoryArgs {
                check,
                json_report: Some(output),
                ..Default::default()
            },
            self.run_spec()?,
        )
    }

    fn discover_mmio(&mut self, check: bool) -> Result<bool> {
        let output = self
            .project
            .registers
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?
            .facts
            .clone();
        let mut arguments = mmio_arguments(self.run_spec()?, self.memory_map()?, &output)?;
        arguments.check = check;
        super::discover_mmio::run(arguments, self.svd)
    }

    fn discover_interfaces(&mut self, check: bool) -> Result<bool> {
        let output = self
            .project
            .interfaces
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("[interfaces] is absent"))?
            .facts
            .clone();
        super::interface_discovery::run(
            InterfaceDiscoverArgs {
                check,
                json_report: Some(output),
                ..Default::default()
            },
            self.run_spec()?,
        )
    }

    fn build_linked_ir(&mut self, check: bool) -> Result<bool> {
        super::ir_build::run(
            IrBuildArgs {
                check,
                ..Default::default()
            },
            self.project,
            self.run_spec()?,
            self.svd,
            self.target,
        )
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

fn mmio_arguments(
    run_spec: &RunSpec,
    memory_map: &MemoryMap,
    output: &Path,
) -> Result<MmioDiscoverArgs> {
    let mut artifacts = Vec::new();
    for input in run_spec.inputs() {
        let InputRole::SourceArtifact(source) = &input.role else {
            continue;
        };
        artifacts.push(
            SourcePath::new(source.clone(), input.path.clone()).map_err(crate::Error::invalid)?,
        );
    }
    let ranges = memory_map
        .mmio_ranges()?
        .into_iter()
        .map(|(name, start, end)| NamedAddressRange { name, start, end })
        .collect();
    Ok(MmioDiscoverArgs {
        artifact: artifacts,
        range: ranges,
        json_report: Some(output.to_owned()),
        ..Default::default()
    })
}
