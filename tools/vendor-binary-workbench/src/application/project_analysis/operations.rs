//! Frontend-neutral project analysis/review operations over domain workspaces.

use crate::{
    MemoryMap, Result, TargetSpec,
    analysis::build_project_linkage_inventory,
    artifacts::{build_symbol_inventory_document, render_symbol_inventory},
    function_workspace::{FunctionWorkspace, link_reviewed_interfaces, render_function_review},
    interfaces::InterfaceWorkspace,
    project::ProjectSpec,
    registers::{
        ProjectRegisterWorkspace, RegisterFacts, RegisterModel, render_register_review,
        validate_pac_api, validate_register_evidence, validate_register_lints,
        validate_register_memory_map,
    },
    run_spec::RunSpec,
};

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

pub(crate) fn validate_registers(
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
    deny_unreviewed: bool,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
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
