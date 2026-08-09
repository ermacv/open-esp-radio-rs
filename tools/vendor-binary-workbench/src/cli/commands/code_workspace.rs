//! Lifecycle commands for human-reviewed executable-code boundaries.

use std::path::Path;

use serde::Serialize;

use super::super::*;
use crate::{
    artifacts::symbol_inventory::load_code_boundary_facts,
    cli::resolver::CodeWorkspaceCommand,
    code_workspace::{
        CodeWorkspace, render_code_boundary_review, write_code_boundary_pack_template,
    },
    project::{CodeWorkspacePaths, ProjectSpec},
};

pub(super) fn run(command: CodeWorkspaceCommand, project: &ProjectSpec) -> Result<bool> {
    let paths = project.code.as_ref().ok_or_else(|| {
        crate::Error::invalid("project has no [code] reviewed boundary workspace")
    })?;
    let inventory = project
        .symbol_inventory
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("project [code] requires [analysis.symbols]"))?;
    match command {
        CodeWorkspaceCommand::InitPack(arguments) => {
            init_pack(arguments, project, paths, &inventory.output)
        }
        CodeWorkspaceCommand::Validate(arguments) => {
            validate(arguments, project, paths, &inventory.output)
        }
        CodeWorkspaceCommand::Review(arguments) => {
            review(arguments, project, paths, &inventory.output)
        }
    }
}

fn init_pack(
    arguments: OutputArgs,
    project: &ProjectSpec,
    paths: &CodeWorkspacePaths,
    inventory: &Path,
) -> Result<bool> {
    let facts = load_code_boundary_facts(inventory)?;
    let output = arguments.output.as_deref().unwrap_or(&paths.pack);
    write_code_boundary_pack_template(output, &facts, &project.id)?;
    let report = CodePackReport {
        schema: 1,
        command: "code init-pack",
        status: "created",
        inputs: facts.inputs.len(),
        candidates: facts.candidates.len(),
        path: output,
    };
    crate::cli::output::render_report(&report, || {
        outputln!("Code-boundary pack: created — {}", output.display());
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Inputs", "Recovery candidates"],
                [[report.inputs.to_string(), report.candidates.to_string()]],
            )
        );
    });
    Ok(true)
}

fn validate(
    arguments: ValidationArgs,
    project: &ProjectSpec,
    paths: &CodeWorkspacePaths,
    inventory: &Path,
) -> Result<bool> {
    let facts = load_code_boundary_facts(inventory)?;
    let workspace = CodeWorkspace::load(&facts, &paths.pack, &project.id)?;
    let summary = workspace.summary();
    let passed = !arguments.deny_unreviewed || summary.unreviewed == 0;
    let report = CodeWorkspaceReport {
        schema: 1,
        command: "code validate",
        status: if passed { "valid" } else { "unreviewed" },
        deny_unreviewed: arguments.deny_unreviewed,
        inputs: summary.inputs,
        candidates: summary.observed_candidates,
        accepted: summary.accepted,
        rejected: summary.rejected,
        unreviewed: summary.unreviewed,
        pack: &paths.pack,
    };
    crate::cli::output::render_report(&report, || print_workspace(&report));
    Ok(passed)
}

fn review(
    arguments: ReviewArgs,
    project: &ProjectSpec,
    paths: &CodeWorkspacePaths,
    inventory: &Path,
) -> Result<bool> {
    let output = arguments
        .output
        .as_deref()
        .or(paths.review_output.as_deref())
        .ok_or_else(|| {
            crate::Error::invalid("code review requires --output or [code.review].output")
        })?;
    let facts = load_code_boundary_facts(inventory)?;
    let workspace = CodeWorkspace::load(&facts, &paths.pack, &project.id)?;
    let contents = render_code_boundary_review(&workspace, inventory)?;
    crate::application::generated_file::write_or_check(
        output,
        &contents,
        arguments.check,
        "code-boundary review",
    )?;
    let summary = workspace.summary();
    let report = CodeReviewReport {
        schema: 1,
        command: "code review",
        status: if arguments.check {
            "verified"
        } else {
            "written"
        },
        candidates: summary.observed_candidates,
        accepted: summary.accepted,
        rejected: summary.rejected,
        unreviewed: summary.unreviewed,
        output,
    };
    crate::cli::output::render_report(&report, || {
        outputln!(
            "Code-boundary review: {} — {}",
            report.status,
            output.display()
        );
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Candidates", "Accepted", "Rejected", "Unreviewed"],
                [[
                    report.candidates.to_string(),
                    report.accepted.to_string(),
                    report.rejected.to_string(),
                    report.unreviewed.to_string(),
                ]],
            )
        );
    });
    Ok(true)
}

fn print_workspace(report: &CodeWorkspaceReport<'_>) {
    outputln!(
        "Code-boundary workspace: {} — {}",
        report.status,
        report.pack.display()
    );
    outputln!(
        "{}",
        crate::cli::table::render(
            ["Candidates", "Accepted", "Rejected", "Unreviewed"],
            [[
                report.candidates.to_string(),
                report.accepted.to_string(),
                report.rejected.to_string(),
                report.unreviewed.to_string(),
            ]],
        )
    );
}

#[derive(Serialize)]
struct CodePackReport<'a> {
    schema: u32,
    command: &'static str,
    status: &'static str,
    inputs: usize,
    candidates: usize,
    path: &'a Path,
}

#[derive(Serialize)]
struct CodeWorkspaceReport<'a> {
    schema: u32,
    command: &'static str,
    status: &'static str,
    deny_unreviewed: bool,
    inputs: usize,
    candidates: usize,
    accepted: usize,
    rejected: usize,
    unreviewed: usize,
    pack: &'a Path,
}

#[derive(Serialize)]
struct CodeReviewReport<'a> {
    schema: u32,
    command: &'static str,
    status: &'static str,
    candidates: usize,
    accepted: usize,
    rejected: usize,
    unreviewed: usize,
    output: &'a Path,
}
