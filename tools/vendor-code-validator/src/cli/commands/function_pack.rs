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

pub(super) fn run(
    command: Command,
    arguments: Vec<String>,
    project: &ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    let paths = project
        .functions
        .as_ref()
        .ok_or("project has no [functions] table; configure a reviewed pack first")?;
    match command {
        Command::FunctionInitPack => init_pack(arguments, project, &paths.pack),
        Command::FunctionValidate => validate(arguments, project, &paths.pack),
        Command::FunctionReview => review(arguments, project, paths, target),
        _ => unreachable!("function pack dispatcher received another command"),
    }
}

fn init_pack(arguments: Vec<String>, project: &ProjectSpec, configured: &Path) -> Result<bool> {
    let mut output = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            _ => return Err(format!("unknown functions init-pack option: {argument}").into()),
        }
    }
    let reports = project.function_ir_reports()?;
    let facts = FunctionFacts::load(&reports)?;
    let output = output.as_deref().unwrap_or(configured);
    write_function_pack_template(output, &facts, &project.id)?;
    println!(
        "FUNCTION-PACK\tstatus=created\tinputs={}\tfunctions={}\troot-functions={}\tcontext-fields={}\tpath={}",
        facts.inputs.len(),
        facts.functions.len(),
        facts.root_functions().count(),
        facts
            .root_functions()
            .map(|function| function.context_fields.len())
            .sum::<usize>(),
        output.display()
    );
    Ok(true)
}

fn validate(arguments: Vec<String>, project: &ProjectSpec, pack: &Path) -> Result<bool> {
    let deny_unreviewed = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--deny-unreviewed" => true,
        _ => return Err(format!("unknown functions validate options: {arguments:?}").into()),
    };
    let reports = project.function_ir_reports()?;
    let workspace = FunctionWorkspace::load(&reports, pack)?;
    let summary = workspace.summary();
    println!(
        "FUNCTION-WORKSPACE\tstatus=valid\tinputs={}\tobserved-functions={}\treviewed-functions={}\tignored-functions={}\tunreviewed-functions={}\treviewed-contexts={}\tignored-contexts={}\tunreviewed-contexts={}\treviewed-fields={}\tignored-fields={}\tunreviewed-fields={}\taccepted-incomplete={}\tpack={}",
        summary.inputs,
        summary.observed_functions,
        summary.reviewed_functions,
        summary.ignored_functions,
        summary.unreviewed_functions,
        summary.reviewed_contexts,
        summary.ignored_contexts,
        summary.unreviewed_contexts,
        summary.reviewed_fields,
        summary.ignored_fields,
        summary.unreviewed_fields,
        summary.accepted_incomplete,
        pack.display(),
    );
    Ok(!deny_unreviewed
        || (summary.unreviewed_functions == 0
            && summary.unreviewed_contexts == 0
            && summary.unreviewed_fields == 0))
}

fn review(
    arguments: Vec<String>,
    project: &ProjectSpec,
    paths: &FunctionWorkspacePaths,
    target: &TargetSpec,
) -> Result<bool> {
    let mut output = None;
    let mut check = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--check" => {
                if check {
                    return Err("duplicate --check".into());
                }
                check = true;
            }
            _ => return Err(format!("unknown functions review option: {argument}").into()),
        }
    }
    let output = output
        .as_deref()
        .or(paths.review_output.as_deref())
        .ok_or("functions review requires --output or [functions.review].output")?;
    let reports = project.function_ir_reports()?;
    let workspace = FunctionWorkspace::load(&reports, &paths.pack)?;
    let interface_links = reviewed_interface_links(project, target, &workspace)?;
    let contents = render_function_review(&workspace, interface_links.as_deref())?;
    super::super::generated_output::write_or_check(output, &contents, check, "function review")?;
    let summary = workspace.summary();
    println!(
        "FUNCTION-REVIEW\tstatus={}\troot-functions={}\treviewed={}\tunreviewed={}\tcontexts={}\tfields={}\tinterface-links={}\toutput={}",
        if check { "verified" } else { "written" },
        summary.observed_functions,
        summary.reviewed_functions,
        summary.unreviewed_functions,
        summary.reviewed_contexts,
        summary.reviewed_fields,
        interface_links.as_ref().map_or(0, Vec::len),
        output.display()
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
