//! Project function/context-pack lifecycle commands.

use std::path::Path;

use super::super::*;
use crate::{
    cli::resolver::FunctionWorkspaceCommand,
    function_workspace::{
        FunctionFacts, FunctionWorkspace, link_reviewed_interfaces, render_function_review,
        write_function_pack_template,
    },
    interfaces::InterfaceWorkspace,
    project::{FunctionWorkspacePaths, ProjectSpec},
};

mod report;

use report::*;

pub(super) fn run(
    command: FunctionWorkspaceCommand,
    project: &ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    let paths = project
        .functions
        .as_ref()
        .ok_or("project has no [functions] table; configure a reviewed pack first")
        .map_err(crate::Error::invalid)?;
    match command {
        FunctionWorkspaceCommand::InitPack(arguments) => init_pack(arguments, project, &paths.pack),
        FunctionWorkspaceCommand::Validate(arguments) => validate(arguments, project, &paths.pack),
        FunctionWorkspaceCommand::Review(arguments) => review(arguments, project, paths, target),
    }
}

fn init_pack(arguments: OutputArgs, project: &ProjectSpec, configured: &Path) -> Result<bool> {
    let reports = project.function_ir_reports()?;
    let facts = FunctionFacts::load(&reports)?;
    let output = arguments.output.as_deref().unwrap_or(configured);
    write_function_pack_template(output, &facts, &project.id)?;
    let report = FunctionPackDocument {
        schema: 2,
        command: "functions init-pack",
        status: "created",
        inputs: facts.inputs.len(),
        functions: facts.functions.len(),
        root_functions: facts.root_functions().count(),
        context_fields: facts
            .root_functions()
            .map(|function| function.context_fields.len())
            .sum::<usize>(),
        path: output,
    };
    crate::cli::output::render_report(&report, || print_pack_human(&report));
    Ok(true)
}

fn validate(arguments: ValidationArgs, project: &ProjectSpec, pack: &Path) -> Result<bool> {
    let reports = project.function_ir_reports()?;
    let workspace = FunctionWorkspace::load(&reports, pack)?;
    let summary = workspace.summary();
    let passed = !arguments.deny_unreviewed
        || (summary.unreviewed_functions == 0
            && summary.unreviewed_contexts == 0
            && summary.unreviewed_fields == 0
            && summary.unreviewed_type_fields == 0);
    let report = FunctionWorkspaceDocument {
        schema: 2,
        command: "functions validate",
        status: if passed { "valid" } else { "unreviewed" },
        deny_unreviewed: arguments.deny_unreviewed,
        inputs: summary.inputs,
        observed_functions: summary.observed_functions,
        reviewed_functions: summary.reviewed_functions,
        ignored_functions: summary.ignored_functions,
        unreviewed_functions: summary.unreviewed_functions,
        reviewed_contexts: summary.reviewed_contexts,
        ignored_contexts: summary.ignored_contexts,
        unreviewed_contexts: summary.unreviewed_contexts,
        reviewed_fields: summary.reviewed_fields,
        ignored_fields: summary.ignored_fields,
        unreviewed_fields: summary.unreviewed_fields,
        accepted_incomplete: summary.accepted_incomplete,
        logical_types: summary.logical_types,
        type_bindings: summary.type_bindings,
        reviewed_type_fields: summary.reviewed_type_fields,
        ignored_type_fields: summary.ignored_type_fields,
        unreviewed_type_fields: summary.unreviewed_type_fields,
        pack,
    };
    crate::cli::output::render_report(&report, || print_workspace_human(&report));
    Ok(passed)
}

fn review(
    arguments: ReviewArgs,
    project: &ProjectSpec,
    paths: &FunctionWorkspacePaths,
    target: &TargetSpec,
) -> Result<bool> {
    let output = arguments
        .output
        .as_deref()
        .or(paths.review_output.as_deref())
        .ok_or("functions review requires --output or [functions.review].output")
        .map_err(crate::Error::invalid)?;
    let reports = project.function_ir_reports()?;
    let workspace = FunctionWorkspace::load(&reports, &paths.pack)?;
    let interface_links = reviewed_interface_links(project, target, &workspace)?;
    let contents = render_function_review(&workspace, interface_links.as_deref())?;
    crate::application::generated_file::write_or_check(
        output,
        &contents,
        arguments.check,
        "function review",
    )?;
    let summary = workspace.summary();
    let report = FunctionReviewDocument {
        schema: 2,
        command: "functions review",
        status: if arguments.check {
            "verified"
        } else {
            "written"
        },
        root_functions: summary.observed_functions,
        reviewed: summary.reviewed_functions,
        unreviewed: summary.unreviewed_functions,
        contexts: summary.reviewed_contexts,
        fields: summary.reviewed_fields,
        interface_links: interface_links.as_ref().map_or(0, Vec::len),
        logical_types: summary.logical_types,
        type_bindings: summary.type_bindings,
        output,
    };
    crate::cli::output::render_report(&report, || print_review_human(&report));
    Ok(true)
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
    let interfaces = InterfaceWorkspace::load_with_templates(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        &paths.interface_template_packs,
        target.calling_convention.label(),
        target
            .knowledge_provider
            .as_deref()
            .map(crate::providers::contracts)
            .transpose()?,
    )?;
    Ok(Some(link_reviewed_interfaces(
        functions,
        interfaces.bindings(),
    )?))
}
