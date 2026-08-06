//! Project register-workspace lifecycle commands.

use super::super::*;
use crate::{project::ProjectSpec, registers::*};

mod publication;

use publication::{export_svd, generate_bindings, generate_pac_source};

pub(super) fn run(
    command: Command,
    arguments: Vec<String>,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or("project has no [registers] table; configure facts and model paths first")?;
    match command {
        Command::RegisterInitOverlay => init_overlay(arguments, project, paths),
        Command::RegisterInitModel => init_model(arguments, project, memory_map, paths),
        Command::RegisterImportSvd => import_svd(arguments, memory_map, paths),
        Command::RegisterValidate => validate(arguments, memory_map, paths),
        Command::RegisterReview => review(arguments, paths),
        Command::RegisterExportSvd => export_svd(arguments, paths),
        Command::RegisterGeneratePac => generate_pac_source(arguments, paths),
        Command::RegisterGenerateBindings => generate_bindings(arguments, paths),
        _ => unreachable!("register command dispatcher received another command"),
    }
}

fn review(arguments: Vec<String>, paths: &crate::project::RegisterWorkspacePaths) -> Result<bool> {
    let mut output = None;
    let mut ir_reports = Vec::new();
    let mut no_ir_reports = false;
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
            "--ir-report" => {
                if no_ir_reports {
                    return Err("--ir-report conflicts with --no-ir-reports".into());
                }
                ir_reports.push(PathBuf::from(take_value(&mut arguments, "--ir-report")?))
            }
            "--no-ir-reports" => {
                if no_ir_reports {
                    return Err("duplicate --no-ir-reports".into());
                }
                if !ir_reports.is_empty() {
                    return Err("--no-ir-reports conflicts with --ir-report".into());
                }
                no_ir_reports = true;
            }
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
    if ir_reports.is_empty() && !no_ir_reports {
        ir_reports.clone_from(&paths.review_ir_reports);
    }
    let (contents, summary) =
        render_register_review(&facts, &model, &ir_reports, &paths.facts, &paths.model)?;
    super::super::generated_output::write_or_check(output, &contents, check, "register review")?;
    println!(
        "REGISTER-REVIEW\tstatus={}\tobserved={}\treviewed={}\tunreviewed={}\tmodel-only={}\tdraft-field-partitions={}\tir-reports={}\tir-registers={}\tir-only-registers={}\tir-field-candidates={}\tpath={}",
        if check { "verified" } else { "written" },
        summary.observed,
        summary.reviewed,
        summary.unreviewed,
        summary.model_only,
        summary.field_candidates,
        summary.ir_reports,
        summary.ir_registers,
        summary.ir_only_registers,
        summary.ir_field_candidates,
        output.display()
    );
    Ok(true)
}

fn init_model(
    arguments: Vec<String>,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
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
        None => memory_map
            .map(|memory| memory.default_address_space.clone())
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
    memory_map: Option<&MemoryMap>,
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
        None => memory_map
            .map(|memory| memory.default_address_space.clone())
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
    memory_map: Option<&MemoryMap>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let deny_unreviewed = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--deny-unreviewed" => true,
        _ => return Err(format!("unknown registers validate options: {arguments:?}").into()),
    };
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let summary = print_summary(&workspace, paths)?;
    let api_pack = validate_pac_api(paths)?;
    if let Some(pack) = &api_pack {
        println!(
            "PAC-API\tstatus=valid\tschema={}\toperations={}\tsources={}\tpack={}",
            pack.schema,
            pack.operation_count(),
            pack.source_ids().len(),
            paths
                .api_pack
                .as_deref()
                .expect("loaded API pack has a configured path")
                .display()
        );
    }
    if let Some(pack) = validate_register_lints(paths)? {
        println!(
            "REGISTER-LINTS\tstatus=valid\tschema={}\tforbidden-field-name-substrings={}\tpack={}",
            pack.schema,
            pack.forbidden_field_name_substrings.len(),
            paths
                .lint_pack
                .as_deref()
                .expect("validated lint pack has a configured path")
                .display()
        );
    }
    if let Some(evidence) = validate_register_evidence(paths, memory_map)? {
        println!(
            "REGISTER-EVIDENCE\tstatus=valid\tcatalogs={}\tconfidence-levels={}\tsources={}\tranges={}",
            paths.evidence_catalogs.len(),
            evidence.confidence_levels.len(),
            evidence.sources.len(),
            evidence.ranges.len()
        );
    }
    if deny_unreviewed && summary.unreviewed != 0 {
        eprintln!(
            "REGISTER-WORKSPACE\tstatus=unreviewed\tcount={}",
            summary.unreviewed
        );
        return Ok(false);
    }
    Ok(true)
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
