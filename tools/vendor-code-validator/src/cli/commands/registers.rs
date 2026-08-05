//! Project register-workspace lifecycle commands.

use std::{fs, path::Path};

use super::super::*;
use crate::{project::ProjectSpec, registers::*};

pub(super) fn run(command: Command, arguments: Vec<String>, project: &ProjectSpec) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or("project has no [registers] table; configure facts and model paths first")?;
    match command {
        Command::RegisterInitOverlay => init_overlay(arguments, project, paths),
        Command::RegisterInitModel => init_model(arguments, project, paths),
        Command::RegisterImportSvd => import_svd(arguments, project, paths),
        Command::RegisterValidate => validate(arguments, paths),
        Command::RegisterReview => review(arguments, paths),
        Command::RegisterExportSvd => export_svd(arguments, paths),
        Command::RegisterGeneratePac => generate_pac_source(arguments, paths),
        _ => unreachable!("register command dispatcher received another command"),
    }
}

fn review(arguments: Vec<String>, paths: &crate::project::RegisterWorkspacePaths) -> Result<bool> {
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
            "--check" => check = true,
            _ => return Err(format!("unknown registers review option: {argument}").into()),
        }
    }
    let output = output
        .as_deref()
        .or(paths.review_output.as_deref())
        .ok_or("registers review requires --output PATH or [registers.review] output")?;
    if !RegisterModel::is_model_file(&paths.model)? {
        return Err(
            "registers review requires a schema 2 model; migrate a legacy overlay first".into(),
        );
    }
    let facts = RegisterFacts::load(&paths.facts)?;
    let model = RegisterModel::load(&paths.model)?;
    let (contents, summary) = render_register_review(&facts, &model, &paths.facts, &paths.model)?;
    write_or_check(output, &contents, check, "register review")?;
    println!(
        "REGISTER-REVIEW\tstatus={}\tobserved={}\treviewed={}\tunreviewed={}\tmodel-only={}\tfield-candidates={}\tpath={}",
        if check { "verified" } else { "written" },
        summary.observed,
        summary.reviewed,
        summary.unreviewed,
        summary.model_only,
        summary.field_candidates,
        output.display()
    );
    Ok(true)
}

fn init_model(
    arguments: Vec<String>,
    project: &ProjectSpec,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut address_space = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--address-space" => {
                if address_space.is_some() {
                    return Err("duplicate --address-space".into());
                }
                address_space = Some(take_value(&mut arguments, "--address-space")?);
            }
            _ => return Err(format!("unknown registers init-model option: {argument}").into()),
        }
    }
    let output = output.as_deref().unwrap_or(&paths.model);
    let address_space = match address_space {
        Some(address_space) => address_space,
        None => project
            .load_memory_map()?
            .map(|memory| memory.default_address_space)
            .unwrap_or_else(|| "cpu".to_owned()),
    };
    let facts = RegisterFacts::load(&paths.facts)?;
    let summary = init_register_model(&facts, output, &address_space, &project.id)?;
    println!(
        "REGISTER-MODEL\tstatus=created\tschema=2\tperipherals={}\tfragments={}\tobserved-registers={}\taddress-space={}\tmodel={}",
        summary.peripherals,
        summary.fragments,
        facts.registers.len(),
        address_space,
        output.display()
    );
    Ok(true)
}

fn import_svd(
    arguments: Vec<String>,
    project: &ProjectSpec,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut input = None;
    let mut output = None;
    let mut address_space = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--input" => {
                if input.is_some() {
                    return Err("duplicate --input".into());
                }
                input = Some(PathBuf::from(take_value(&mut arguments, "--input")?));
            }
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--address-space" => {
                if address_space.is_some() {
                    return Err("duplicate --address-space".into());
                }
                address_space = Some(take_value(&mut arguments, "--address-space")?);
            }
            _ => return Err(format!("unknown registers import-svd option: {argument}").into()),
        }
    }
    let input = input.ok_or("registers import-svd requires --input PATH")?;
    let output = output.as_deref().unwrap_or(&paths.model);
    let address_space = match address_space {
        Some(address_space) => address_space,
        None => project
            .load_memory_map()?
            .map(|memory| memory.default_address_space)
            .unwrap_or_else(|| "cpu".to_owned()),
    };
    let summary = import_svd_model(&input, output, &address_space)?;
    println!(
        "REGISTER-MODEL\tstatus=imported\tschema=2\tperipherals={}\tfragments={}\tannotations={}\taddress-space={}\tinput={}\tmodel={}",
        summary.peripherals,
        summary.fragments,
        summary.annotations,
        address_space,
        input.display(),
        output.display()
    );
    Ok(true)
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
    let output = output.as_deref().unwrap_or(&paths.model);
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
    let deny_unreviewed = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--deny-unreviewed" => true,
        _ => return Err(format!("unknown registers validate options: {arguments:?}").into()),
    };
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let summary = print_summary(&workspace, paths)?;
    if deny_unreviewed && summary.unreviewed != 0 {
        eprintln!(
            "REGISTER-WORKSPACE\tstatus=unreviewed\tcount={}",
            summary.unreviewed
        );
        return Ok(false);
    }
    Ok(true)
}

fn export_svd(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut profile = SvdExportProfile::Release;
    let mut check = false;
    let mut deny_unreviewed = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--profile" => {
                profile = match take_value(&mut arguments, "--profile")?.as_str() {
                    "audit" => SvdExportProfile::Audit,
                    "release" => SvdExportProfile::Release,
                    value => {
                        return Err(format!(
                            "registers export-svd profile must be \"audit\" or \"release\", got {value:?}"
                        )
                        .into());
                    }
                };
            }
            "--reviewed-only" => profile = SvdExportProfile::Release,
            "--deny-unreviewed" => deny_unreviewed = true,
            "--check" => check = true,
            _ => return Err(format!("unknown registers export-svd option: {argument}").into()),
        }
    }
    let output = output
        .as_deref()
        .or(paths.svd_output.as_deref())
        .ok_or("registers export-svd requires --output PATH or [registers.svd] output")?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = print_summary(&workspace, paths)?;
    if deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(format!(
            "release SVD denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )
        .into());
    }
    let (contents, summary) = workspace.render_svd(profile)?;
    write_or_check(output, &contents, check, "SVD")?;
    println!(
        "SVD\tstatus={}\tprofile={}\tperipherals={}\tregisters={}\tfields={}\tpath={}",
        if check { "verified" } else { "written" },
        profile.label(),
        summary.peripherals,
        summary.registers,
        summary.fields,
        output.display()
    );
    Ok(true)
}

fn generate_pac_source(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut target = None;
    let mut edition = None;
    let mut check = false;
    let mut deny_unreviewed = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--target" => {
                target = Some(PacTarget::parse(&take_value(&mut arguments, "--target")?)?);
            }
            "--edition" => {
                edition = Some(PacEdition::parse(&take_value(
                    &mut arguments,
                    "--edition",
                )?)?);
            }
            "--check" => check = true,
            "--deny-unreviewed" => deny_unreviewed = true,
            _ => return Err(format!("unknown registers generate-pac option: {argument}").into()),
        }
    }
    let configured = paths.pac.as_ref();
    let output = output
        .as_deref()
        .or_else(|| configured.map(|pac| pac.output.as_path()))
        .ok_or("registers generate-pac requires --output PATH or [registers.pac] output")?;
    let target = target
        .map(Ok)
        .unwrap_or_else(|| PacTarget::parse(configured.map_or("none", |pac| &pac.target)))?;
    let edition = edition
        .map(Ok)
        .unwrap_or_else(|| PacEdition::parse(configured.map_or("2024", |pac| &pac.edition)))?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = print_summary(&workspace, paths)?;
    if deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(format!(
            "PAC generation denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )
        .into());
    }
    let (svd, svd_summary) = workspace.render_svd(SvdExportProfile::Release)?;
    let source = generate_pac(&svd, target, edition)?;
    write_or_check(output, &source, check, "PAC")?;
    println!(
        "PAC\tstatus={}\ttarget={}\tedition={}\tperipherals={}\tregisters={}\tpath={}",
        if check { "verified" } else { "written" },
        target.label(),
        edition.label(),
        svd_summary.peripherals,
        svd_summary.registers,
        output.display()
    );
    Ok(true)
}

fn write_or_check(path: &Path, contents: &str, check: bool, kind: &str) -> Result<()> {
    if check {
        let existing = fs::read_to_string(path).map_err(|error| {
            format!("cannot check generated {kind} {}: {error}", path.display())
        })?;
        if existing != contents {
            return Err(format!(
                "generated {kind} differs from {}; rerun without --check",
                path.display()
            )
            .into());
        }
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn print_summary(
    workspace: &ProjectRegisterWorkspace,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<RegisterWorkspaceSummary> {
    let summary = workspace.summary()?;
    println!(
        "REGISTER-WORKSPACE\tstatus=valid\tformat={}\tranges={}\tobserved={}\treviewed={}\tignored={}\tmanual={}\tunreviewed={}\tfields={}\tfacts={}\tmodel={}",
        workspace.format_label(),
        summary.ranges,
        summary.observed,
        summary.reviewed,
        summary.ignored,
        summary.manual,
        summary.unreviewed,
        summary.fields,
        paths.facts.display(),
        paths.model.display()
    );
    Ok(summary)
}
