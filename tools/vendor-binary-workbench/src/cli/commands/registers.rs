//! Project register-workspace lifecycle commands.

use super::super::*;
use crate::{project::ProjectSpec, registers::*};

mod publication;

pub(super) use publication::{
    PreparedPublication, PublicationReadiness, prepare_project_bindings, prepare_project_pac,
    prepare_project_svd,
};
use publication::{export_svd, generate_bindings, generate_pac_source};

pub(super) fn run(
    command: Command,
    arguments: CommandArguments,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or("project has no [registers] table; configure facts and model paths first")?;
    match (command, arguments) {
        (Command::RegisterInitModel, CommandArguments::RegisterModel(arguments)) => {
            init_model(arguments, project, memory_map, paths)
        }
        (Command::RegisterImportSvd, CommandArguments::RegisterImport(arguments)) => {
            import_svd(arguments, memory_map, paths)
        }
        (Command::RegisterValidate, CommandArguments::Validation(arguments)) => {
            validate(arguments, memory_map, paths)
        }
        (Command::RegisterReview, CommandArguments::RegisterReview(arguments)) => {
            review(arguments, paths)
        }
        (Command::RegisterExportSvd, CommandArguments::RegisterExport(arguments)) => {
            export_svd(arguments, paths)
        }
        (Command::RegisterGeneratePac, CommandArguments::RegisterPac(arguments)) => {
            generate_pac_source(arguments, paths)
        }
        (Command::RegisterGenerateBindings, CommandArguments::RegisterBindings(arguments)) => {
            generate_bindings(arguments, paths)
        }
        _ => unreachable!("register command dispatcher received another command"),
    }
}

fn review(
    arguments: RegisterReviewArgs,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let output = arguments
        .output
        .as_deref()
        .or(paths.review_output.as_deref())
        .ok_or("registers review requires --output PATH or [registers.review] output")?;
    if !RegisterModel::is_model_file(&paths.model)? {
        return Err("registers review requires a register-model-v2 manifest".into());
    }
    let facts = RegisterFacts::load(&paths.facts)?;
    let model = RegisterModel::load(&paths.model)?;
    let mut ir_reports = arguments.ir_report;
    if ir_reports.is_empty() && !arguments.no_ir_reports {
        ir_reports.clone_from(&paths.review_ir_reports);
    }
    let (contents, summary) =
        render_register_review(&facts, &model, &ir_reports, &paths.facts, &paths.model)?;
    super::super::generated_output::write_or_check(
        output,
        &contents,
        arguments.check,
        "register review",
    )?;
    outputln!(
        "REGISTER-REVIEW\tstatus={}\tobserved={}\treviewed={}\tunreviewed={}\tmodel-only={}\tdraft-field-partitions={}\tir-reports={}\tir-registers={}\tir-only-registers={}\tir-field-candidates={}\tpath={}",
        if arguments.check {
            "verified"
        } else {
            "written"
        },
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
    arguments: RegisterModelArgs,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let output = arguments.output.as_deref().unwrap_or(&paths.model);
    let address_space = match arguments.address_space {
        Some(address_space) => address_space,
        None => memory_map
            .map(|memory| memory.default_address_space.clone())
            .unwrap_or_else(|| "cpu".to_owned()),
    };
    let facts = RegisterFacts::load(&paths.facts)?;
    let summary = init_register_model(&facts, output, &address_space, &project.id)?;
    outputln!(
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
    arguments: RegisterImportArgs,
    memory_map: Option<&MemoryMap>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let input = arguments.input;
    let output = arguments.output.as_deref().unwrap_or(&paths.model);
    let address_space = match arguments.address_space {
        Some(address_space) => address_space,
        None => memory_map
            .map(|memory| memory.default_address_space.clone())
            .unwrap_or_else(|| "cpu".to_owned()),
    };
    let summary = import_svd_model(&input, output, &address_space)?;
    outputln!(
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

fn validate(
    arguments: ValidationArgs,
    memory_map: Option<&MemoryMap>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let summary = print_summary(&workspace, paths)?;
    let api_pack = validate_pac_api(paths)?;
    if let Some(pack) = &api_pack {
        outputln!(
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
        outputln!(
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
    if let Some(memory) = validate_register_memory_map(paths, memory_map)? {
        outputln!(
            "REGISTER-MEMORY\tstatus=valid\tregisters={}\tmmio-regions={}",
            memory.registers,
            memory.mmio_regions
        );
    }
    if let Some(evidence) = validate_register_evidence(paths, memory_map)? {
        outputln!(
            "REGISTER-EVIDENCE\tstatus=valid\tcatalogs={}\tconfidence-levels={}\tsources={}\tranges={}",
            paths.evidence_catalogs.len(),
            evidence.confidence_levels.len(),
            evidence.sources.len(),
            evidence.ranges.len()
        );
    }
    if arguments.deny_unreviewed && summary.unreviewed != 0 {
        tracing::warn!(
            unreviewed = summary.unreviewed,
            "register workspace contains unreviewed entries"
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
    outputln!(
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
