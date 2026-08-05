//! Project register-workspace lifecycle commands.

use super::super::*;
use crate::{project::ProjectSpec, registers::*};

pub(super) fn run(command: Command, arguments: Vec<String>, project: &ProjectSpec) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or("project has no [registers] table; configure facts and overlay paths first")?;
    match command {
        Command::RegisterInitOverlay => init_overlay(arguments, project, paths),
        Command::RegisterValidate => validate(arguments, paths),
        Command::RegisterExportSvd => export_svd(arguments, paths),
        _ => unreachable!("register command dispatcher received another command"),
    }
}

fn init_overlay(
    arguments: Vec<String>,
    project: &ProjectSpec,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
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
            _ => return Err(format!("unknown registers init-overlay option: {argument}").into()),
        }
    }
    let facts = RegisterFacts::load(&paths.facts)?;
    let output = output.as_deref().unwrap_or(&paths.overlay);
    write_overlay_template(output, &facts, &project.id)?;
    println!(
        "REGISTER-OVERLAY\tstatus=created\tranges={}\tobserved-registers={}\tpath={}",
        facts.ranges.len(),
        facts.registers.len(),
        output.display()
    );
    Ok(true)
}

fn validate(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    if !arguments.is_empty() {
        return Err(format!("registers validate takes no options: {arguments:?}").into());
    }
    let workspace = RegisterWorkspace::load(&paths.facts, &paths.overlay)?;
    print_summary(&workspace, paths);
    Ok(true)
}

fn export_svd(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut reviewed_only = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--reviewed-only" => reviewed_only = true,
            _ => return Err(format!("unknown registers export-svd option: {argument}").into()),
        }
    }
    let output = output.ok_or("registers export-svd requires --output PATH")?;
    let workspace = RegisterWorkspace::load(&paths.facts, &paths.overlay)?;
    print_summary(&workspace, paths);
    let summary = workspace.write_svd(&output, reviewed_only)?;
    println!(
        "SVD\tstatus=written\tmode={}\tperipherals={}\tregisters={}\tfields={}\tpath={}",
        if reviewed_only {
            "reviewed-only"
        } else {
            "merged"
        },
        summary.peripherals,
        summary.registers,
        summary.fields,
        output.display()
    );
    Ok(true)
}

fn print_summary(workspace: &RegisterWorkspace, paths: &crate::project::RegisterWorkspacePaths) {
    let summary = workspace.summary();
    println!(
        "REGISTER-WORKSPACE\tstatus=valid\tranges={}\tobserved={}\treviewed={}\tignored={}\tmanual={}\tunreviewed={}\tfields={}\tfacts={}\toverlay={}",
        summary.ranges,
        summary.observed,
        summary.reviewed,
        summary.ignored,
        summary.manual,
        summary.unreviewed,
        summary.fields,
        paths.facts.display(),
        paths.overlay.display()
    );
}
