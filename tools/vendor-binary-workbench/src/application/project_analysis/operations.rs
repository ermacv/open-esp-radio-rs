//! Frontend-neutral project analysis/review operations over domain workspaces.

use crate::{
    MemoryMap, MmioMap, Result, TargetSpec,
    analysis::{
        DiscoveryRange, EffectiveCodeCatalog, ProjectInterfaceDiscoveryOptions,
        build_project_linkage_inventory, discover_mmio, discover_project_interfaces,
    },
    artifact,
    artifacts::{
        build_interface_facts, build_mmio_facts as mmio_document, build_symbol_inventory_document,
        render_interface_facts, render_mmio_facts, render_symbol_inventory,
    },
    code_workspace::{CodeWorkspace, render_code_boundary_review},
    function_workspace::{FunctionWorkspace, link_reviewed_interfaces, render_function_review},
    interfaces::InterfaceWorkspace,
    project::ProjectSpec,
    registers::{
        ProjectRegisterWorkspace, RegisterFacts, RegisterModel, render_register_review,
        validate_pac_api, validate_register_evidence, validate_register_lints,
        validate_register_memory_map,
    },
    run_spec::{InputRole, RunSpec},
};

use super::{
    ProjectAnalysisInputs, ProjectAnalysisOperations, ProjectAnalysisReport, ProjectAnalysisRequest,
};

pub(crate) fn analyze_project(
    project: &ProjectSpec,
    request: ProjectAnalysisRequest,
    run_spec: Option<&RunSpec>,
    memory_map: Option<&MemoryMap>,
    svd: &MmioMap,
    target: &TargetSpec,
) -> ProjectAnalysisReport {
    let inputs = ProjectAnalysisInputs {
        run_spec: run_spec.is_some(),
        memory_map: memory_map.is_some(),
    };
    let mut operations = ResolvedProjectAnalysisOperations {
        project,
        run_spec,
        memory_map,
        svd,
        target,
    };
    super::run(project, request, inputs, &mut operations)
}

struct ResolvedProjectAnalysisOperations<'a> {
    project: &'a ProjectSpec,
    run_spec: Option<&'a RunSpec>,
    memory_map: Option<&'a MemoryMap>,
    svd: &'a MmioMap,
    target: &'a TargetSpec,
}

impl ResolvedProjectAnalysisOperations<'_> {
    fn run_spec(&self) -> Result<&RunSpec> {
        self.run_spec
            .ok_or_else(|| crate::Error::invalid("run-spec is not configured"))
    }

    fn memory_map(&self) -> Result<&MemoryMap> {
        self.memory_map
            .ok_or_else(|| crate::Error::invalid("memory-map is not configured"))
    }
}

impl ProjectAnalysisOperations for ResolvedProjectAnalysisOperations<'_> {
    fn symbol_inventory(&mut self, check: bool) -> Result<bool> {
        build_symbol_inventory(self.project, self.run_spec()?, check)
    }

    fn discover_mmio(&mut self, check: bool, jobs: usize) -> Result<bool> {
        discover_project_mmio(
            self.project,
            self.run_spec()?,
            self.memory_map()?,
            self.svd,
            check,
            jobs,
        )
    }

    fn discover_interfaces(&mut self, check: bool) -> Result<bool> {
        discover_project_interfaces_operation(self.project, self.run_spec()?, check)
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
        build_navigation(self.project, check)
    }

    fn validate_code(&mut self, deny_unreviewed: bool) -> Result<bool> {
        validate_code_boundaries(self.project, deny_unreviewed)
    }

    fn review_code(&mut self, check: bool) -> Result<bool> {
        review_code_boundaries(self.project, check)
    }

    fn validate_registers(&mut self, deny_unreviewed: bool) -> Result<bool> {
        validate_registers(self.project, self.memory_map, deny_unreviewed)
    }

    fn review_registers(&mut self, check: bool) -> Result<bool> {
        review_registers(self.project, check)
    }

    fn validate_functions(&mut self, deny_unreviewed: bool) -> Result<bool> {
        validate_functions(self.project, deny_unreviewed)
    }

    fn review_functions(&mut self, check: bool) -> Result<bool> {
        review_functions(self.project, self.target, check)
    }

    fn validate_interfaces(&mut self, deny_unreviewed: bool) -> Result<bool> {
        validate_interfaces(self.project, self.target, deny_unreviewed)
    }
}

pub(crate) fn build_symbol_inventory(
    project: &ProjectSpec,
    run_spec: &RunSpec,
    check: bool,
) -> Result<bool> {
    let output = &project
        .symbol_inventory
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.symbols] is absent"))?
        .output;
    let inputs = run_spec
        .inputs()
        .iter()
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect::<Vec<_>>();
    let inventory = build_project_linkage_inventory(&inputs)?;
    let document = build_symbol_inventory_document(&inventory, |_| true)?;
    super::super::generated_file::write_or_check(
        output,
        &render_symbol_inventory(&document)?,
        check,
        "symbol inventory",
    )?;
    Ok(true)
}

pub(crate) fn discover_project_mmio(
    project: &ProjectSpec,
    run_spec: &RunSpec,
    memory_map: &MemoryMap,
    svd: &MmioMap,
    check: bool,
    jobs: usize,
) -> Result<bool> {
    let output = &project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?
        .facts;
    let artifacts = run_spec
        .inputs()
        .iter()
        .filter_map(|input| match &input.role {
            InputRole::SourceArtifact(source) => Some((source.to_string(), input.path.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let ranges = memory_map
        .mmio_ranges()?
        .into_iter()
        .map(|(name, start, end)| DiscoveryRange { name, start, end })
        .collect::<Vec<_>>();
    let report = discover_mmio(
        &artifacts,
        &ranges,
        "",
        artifact::CodeSymbolSelection::All,
        svd,
        Some(&EffectiveCodeCatalog::load(project)?),
        crate::analysis::MmioDiscoveryOptions { jobs },
    )?;
    let document = mmio_document(&report)?;
    super::super::generated_file::write_or_check(
        output,
        &render_mmio_facts(&document)?,
        check,
        "MMIO discovery report",
    )?;
    Ok(true)
}

pub(crate) fn discover_project_interfaces_operation(
    project: &ProjectSpec,
    run_spec: &RunSpec,
    check: bool,
) -> Result<bool> {
    let output = &project
        .interfaces
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[interfaces] is absent"))?
        .facts;
    let inputs = run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_scannable())
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect::<Vec<_>>();
    if inputs.is_empty() {
        return Err(crate::Error::invalid(
            "run spec has no artifact or inventory inputs for interface discovery",
        ));
    }
    let discovery = discover_project_interfaces(
        &inputs,
        &ProjectInterfaceDiscoveryOptions::default(),
        Some(&EffectiveCodeCatalog::load(project)?),
    )?;
    let document = build_interface_facts(&discovery)?;
    super::super::generated_file::write_or_check(
        output,
        &render_interface_facts(&document)?,
        check,
        "interface discovery report",
    )?;
    if !discovery.failures.is_empty() {
        tracing::warn!(
            decode_failures = discovery.failures.len(),
            "interface discovery retained partial findings"
        );
    }
    Ok(true)
}

pub(crate) fn build_navigation(project: &ProjectSpec, check: bool) -> Result<bool> {
    let output = &project
        .navigation_index
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.navigation] is absent"))?
        .output;
    let document = crate::navigation::build(project)?;
    let rendered = serde_json::to_string_pretty(&document)? + "\n";
    super::super::generated_file::write_or_check(output, &rendered, check, "navigation index")?;
    Ok(true)
}

pub(crate) fn validate_code_boundaries(
    project: &ProjectSpec,
    deny_unreviewed: bool,
) -> Result<bool> {
    let paths = project
        .code
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[code] is absent"))?;
    let inventory = &project
        .symbol_inventory
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.symbols] is absent"))?
        .output;
    let facts = crate::artifacts::symbol_inventory::load_code_boundary_facts(inventory)?;
    let workspace = CodeWorkspace::load(&facts, &paths.pack, &project.id)?;
    Ok(!deny_unreviewed || workspace.summary().unreviewed == 0)
}

pub(crate) fn review_code_boundaries(project: &ProjectSpec, check: bool) -> Result<bool> {
    let paths = project
        .code
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[code] is absent"))?;
    let output = paths
        .review_output
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[code.review] is absent"))?;
    let inventory = &project
        .symbol_inventory
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.symbols] is absent"))?
        .output;
    let facts = crate::artifacts::symbol_inventory::load_code_boundary_facts(inventory)?;
    let workspace = CodeWorkspace::load(&facts, &paths.pack, &project.id)?;
    let contents = render_code_boundary_review(&workspace, inventory)?;
    super::super::generated_file::write_or_check(output, &contents, check, "code-boundary review")?;
    Ok(true)
}

pub(crate) fn validate_registers(
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
    deny_unreviewed: bool,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?;
    let workspace = ProjectRegisterWorkspace::load(paths)?;
    let summary = workspace.summary()?;
    validate_pac_api(paths)?;
    validate_register_lints(paths)?;
    validate_register_memory_map(paths, memory_map)?;
    validate_register_evidence(paths, memory_map)?;
    Ok(!deny_unreviewed || summary.unreviewed == 0)
}

pub(crate) fn review_registers(project: &ProjectSpec, check: bool) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?;
    let output = paths
        .review_output
        .as_deref()
        .ok_or_else(|| crate::Error::invalid("[registers.review] is absent"))?;
    if !RegisterModel::is_model_file(&paths.model)? {
        return Err(crate::Error::invalid(
            "registers review requires a register-model-v2 manifest",
        ));
    }
    let facts = RegisterFacts::load(&paths.facts)?;
    let model = RegisterModel::load(&paths.model)?;
    let (contents, _) = render_register_review(
        &facts,
        &model,
        &paths.review_ir_reports,
        &paths.owned_ranges,
        &paths.facts,
        &paths.model,
    )?;
    super::super::generated_file::write_or_check(output, &contents, check, "register review")?;
    Ok(true)
}

pub(crate) fn validate_functions(project: &ProjectSpec, deny_unreviewed: bool) -> Result<bool> {
    let paths = project
        .functions
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[functions] is absent"))?;
    let reports = project.function_ir_reports()?;
    let summary = FunctionWorkspace::load(&reports, &paths.pack)?.summary();
    Ok(!deny_unreviewed
        || (summary.unreviewed_functions == 0
            && summary.unreviewed_contexts == 0
            && summary.unreviewed_fields == 0
            && summary.unreviewed_type_fields == 0))
}

pub(crate) fn review_functions(
    project: &ProjectSpec,
    target: &TargetSpec,
    check: bool,
) -> Result<bool> {
    let paths = project
        .functions
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[functions] is absent"))?;
    let output = paths
        .review_output
        .as_deref()
        .ok_or_else(|| crate::Error::invalid("[functions.review] is absent"))?;
    let reports = project.function_ir_reports()?;
    let workspace = FunctionWorkspace::load(&reports, &paths.pack)?;
    let interface_links = reviewed_interface_links(project, target, &workspace)?;
    let contents = render_function_review(&workspace, interface_links.as_deref())?;
    super::super::generated_file::write_or_check(output, &contents, check, "function review")?;
    Ok(true)
}

pub(crate) fn validate_interfaces(
    project: &ProjectSpec,
    target: &TargetSpec,
    deny_unreviewed: bool,
) -> Result<bool> {
    let paths = project
        .interfaces
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[interfaces] is absent"))?;
    let pack = paths
        .pack
        .as_deref()
        .ok_or_else(|| crate::Error::invalid("[interfaces].pack is absent"))?;
    let workspace = InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        target.calling_convention.label(),
        target
            .harness
            .as_deref()
            .map(crate::harnesses::contracts)
            .transpose()?,
    )?;
    let summary = workspace.summary();
    Ok(!deny_unreviewed || (summary.unreviewed_anchors == 0 && summary.unreviewed_slots == 0))
}

fn reviewed_interface_links(
    project: &ProjectSpec,
    target: &TargetSpec,
    functions: &FunctionWorkspace,
) -> Result<Option<Vec<crate::function_workspace::FunctionInterfaceLink>>> {
    let Some(paths) = project.interfaces.as_ref() else {
        return Ok(None);
    };
    let Some(pack) = paths.pack.as_deref().filter(|pack| pack.is_file()) else {
        return Ok(None);
    };
    let interfaces = InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        target.calling_convention.label(),
        target
            .harness
            .as_deref()
            .map(crate::harnesses::contracts)
            .transpose()?,
    )?;
    Ok(Some(link_reviewed_interfaces(
        functions,
        interfaces.bindings(),
    )?))
}
