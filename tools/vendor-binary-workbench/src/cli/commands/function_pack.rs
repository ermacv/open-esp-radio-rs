//! Project function/context-pack lifecycle commands.

use std::path::Path;

use super::super::*;
use crate::{
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
    command: Command,
    arguments: CommandArguments,
    project: &ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    let paths = project
        .functions
        .as_ref()
        .ok_or("project has no [functions] table; configure a reviewed pack first")?;
    match (command, arguments) {
        (Command::FunctionInitPack, CommandArguments::Output(arguments)) => {
            init_pack(arguments, project, &paths.pack)
        }
        (Command::FunctionValidate, CommandArguments::Validation(arguments)) => {
            validate(arguments, project, &paths.pack)
        }
        (Command::FunctionReview, CommandArguments::Review(arguments)) => {
            review(arguments, project, paths, target)
        }
        _ => unreachable!("function pack dispatcher received another command"),
    }
}

fn init_pack(arguments: OutputArgs, project: &ProjectSpec, configured: &Path) -> Result<bool> {
    let reports = project.function_ir_reports()?;
    let facts = FunctionFacts::load(&reports)?;
    let output = arguments.output.as_deref().unwrap_or(configured);
    write_function_pack_template(output, &facts, &project.id)?;
    let report = FunctionPackDocument {
        schema: 1,
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
    crate::cli::output::render_report(
        &report,
        || print_pack_human(&report),
        || print_pack_tsv(&report),
    );
    Ok(true)
}

fn validate(arguments: ValidationArgs, project: &ProjectSpec, pack: &Path) -> Result<bool> {
    let reports = project.function_ir_reports()?;
    let workspace = FunctionWorkspace::load(&reports, pack)?;
    let summary = workspace.summary();
    let passed = !arguments.deny_unreviewed
        || (summary.unreviewed_functions == 0
            && summary.unreviewed_contexts == 0
            && summary.unreviewed_fields == 0);
    let report = FunctionWorkspaceDocument {
        schema: 1,
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
        pack,
    };
    crate::cli::output::render_report(
        &report,
        || print_workspace_human(&report),
        || print_workspace_tsv(&report),
    );
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
        .ok_or("functions review requires --output or [functions.review].output")?;
    let reports = project.function_ir_reports()?;
    let workspace = FunctionWorkspace::load(&reports, &paths.pack)?;
    let interface_links = reviewed_interface_links(project, target, &workspace)?;
    let contents = render_function_review(&workspace, interface_links.as_deref())?;
    super::super::generated_output::write_or_check(
        output,
        &contents,
        arguments.check,
        "function review",
    )?;
    let summary = workspace.summary();
    let report = FunctionReviewDocument {
        schema: 1,
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
        output,
    };
    crate::cli::output::render_report(
        &report,
        || print_review_human(&report),
        || print_review_tsv(&report),
    );
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
    let interfaces = InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        target.calling_convention.label(),
    )?;
    Ok(Some(link_reviewed_interfaces(
        functions,
        interfaces.bindings(),
    )?))
}
