//! Lifecycle commands for human-reviewed executable-code boundaries.

use std::{fs, path::Path};

use serde::Serialize;

use super::super::*;
use crate::{
    artifacts::symbol_inventory::load_code_boundary_facts,
    cli::resolver::CodeWorkspaceCommand,
    code_workspace::{
        CodeRebaseCandidate, CodeWorkspace, render_code_boundary_review,
        write_code_boundary_pack_template,
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
        CodeWorkspaceCommand::Rebase(arguments) => {
            rebase(arguments, project, paths, &inventory.output)
        }
        CodeWorkspaceCommand::Validate(arguments) => {
            validate(arguments, project, paths, &inventory.output)
        }
        CodeWorkspaceCommand::Review(arguments) => {
            review(arguments, project, paths, &inventory.output)
        }
    }
}

fn rebase(
    arguments: CodeRebaseArgs,
    project: &ProjectSpec,
    paths: &CodeWorkspacePaths,
    inventory: &Path,
) -> Result<bool> {
    let facts = load_code_boundary_facts(inventory)?;
    let candidate = CodeRebaseCandidate::prepare(&facts, &paths.pack, &project.id)?;
    let summary = candidate.summary();
    let (status, output) = if arguments.check {
        (if summary.current { "current" } else { "stale" }, None)
    } else if let Some(output) = arguments.output.as_deref() {
        write_candidate(output, candidate.contents())?;
        ("candidate-written", Some(output))
    } else if arguments.apply {
        if !summary.safe_to_apply {
            return Err(crate::Error::invalid(format!(
                "refusing to update reviewed code-boundary pack: {} added, {} removed and {} changed boundaries; write a review candidate with `code rebase --output PATH`",
                summary.added, summary.removed, summary.changed
            )));
        }
        candidate.validate(&facts, &project.id)?;
        write_atomic(&paths.pack, candidate.contents())?;
        (
            if summary.current {
                "unchanged"
            } else {
                "updated"
            },
            Some(paths.pack.as_path()),
        )
    } else {
        (if summary.current { "current" } else { "stale" }, None)
    };
    let report = CodeRebaseReport {
        schema: 1,
        command: "code rebase",
        status,
        safe_to_apply: summary.safe_to_apply,
        inputs_added: summary.inputs_added,
        inputs_removed: summary.inputs_removed,
        preserved: summary.preserved,
        changed: summary.changed,
        added: summary.added,
        removed: summary.removed,
        pack: &paths.pack,
        output,
    };
    crate::cli::output::render_report(&report, || {
        outputln!("Code-boundary rebase: {status} — {}", paths.pack.display());
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Preserved", "Changed", "Added", "Removed", "Safe apply"],
                [[
                    summary.preserved.to_string(),
                    summary.changed.to_string(),
                    summary.added.to_string(),
                    summary.removed.to_string(),
                    summary.safe_to_apply.to_string(),
                ]],
            )
        );
        if !summary.current && summary.safe_to_apply && !arguments.apply {
            outputln!("Next: rerun with `code rebase --apply` to refresh guards.");
        } else if !summary.safe_to_apply && arguments.output.is_none() {
            outputln!("Next: write a reviewed candidate with `code rebase --output PATH`.");
        }
    });
    Ok(!arguments.check || summary.current)
}

fn write_candidate(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite code-boundary review candidate {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| crate::Error::invalid("code-boundary pack must have a UTF-8 file name"))?;
    let staging = parent.join(format!(
        ".{name}.vendor-workbench-rebase-{}",
        std::process::id()
    ));
    if staging.exists() {
        return Err(crate::Error::invalid(format!(
            "code-boundary staging path exists: {}",
            staging.display()
        )));
    }
    fs::write(&staging, contents)?;
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(error.into());
    }
    Ok(())
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

#[derive(Serialize)]
struct CodeRebaseReport<'a> {
    schema: u32,
    command: &'static str,
    status: &'static str,
    safe_to_apply: bool,
    inputs_added: usize,
    inputs_removed: usize,
    preserved: usize,
    changed: usize,
    added: usize,
    removed: usize,
    pack: &'a Path,
    output: Option<&'a Path>,
}
