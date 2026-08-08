//! Project register-workspace lifecycle commands.

use super::super::*;
use crate::{project::ProjectSpec, registers::*};

mod publication;
mod report;

pub(super) use publication::{
    PreparedPublication, PublicationReadiness, prepare_project_bindings, prepare_project_pac,
    prepare_project_svd,
};
use publication::{export_svd, generate_bindings, generate_pac_source};
use report::*;

pub(super) fn run(
    command: Command,
    arguments: CommandArguments,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or("project has no [registers] table; configure facts and model paths first")
        .map_err(crate::Error::invalid)?;
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
        .ok_or("registers review requires --output PATH or [registers.review] output")
        .map_err(crate::Error::invalid)?;
    if !RegisterModel::is_model_file(&paths.model)? {
        return Err(crate::Error::invalid(
            "registers review requires a register-model-v2 manifest",
        ));
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
    let report = RegisterReviewDocument {
        schema: 1,
        command: "registers review",
        status: if arguments.check {
            "verified"
        } else {
            "written"
        },
        observed: summary.observed,
        reviewed: summary.reviewed,
        unreviewed: summary.unreviewed,
        model_only: summary.model_only,
        draft_field_partitions: summary.field_candidates,
        ir_reports: summary.ir_reports,
        ir_registers: summary.ir_registers,
        ir_only_registers: summary.ir_only_registers,
        ir_field_candidates: summary.ir_field_candidates,
        path: output,
    };
    crate::cli::output::render_report(
        &report,
        || print_review_human(&report),
        || print_review_tsv(&report),
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
    let report = RegisterModelDocument {
        schema: 1,
        command: "registers init-model",
        status: "created",
        model_schema: 2,
        peripherals: summary.peripherals,
        fragments: summary.fragments,
        observed_registers: facts.registers.len(),
        annotations: None,
        address_space: &address_space,
        input: None,
        model: output,
    };
    crate::cli::output::render_report(
        &report,
        || print_model_human(&report),
        || print_model_tsv(&report),
    );
    Ok(true)
}

#[tracing::instrument(name = "import_svd", skip_all, fields(input = %arguments.input.display()))]
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
    let report = RegisterModelDocument {
        schema: 1,
        command: "registers import-svd",
        status: "imported",
        model_schema: 2,
        peripherals: summary.peripherals,
        fragments: summary.fragments,
        observed_registers: 0,
        annotations: Some(summary.annotations),
        address_space: &address_space,
        input: Some(&input),
        model: output,
    };
    crate::cli::output::render_report(
        &report,
        || print_model_human(&report),
        || print_model_tsv(&report),
    );
    Ok(true)
}

fn validate(
    arguments: ValidationArgs,
    memory_map: Option<&MemoryMap>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let summary = workspace.summary()?;
    let api_pack = validate_pac_api(paths)?;
    let lint_pack = validate_register_lints(paths)?;
    let memory = validate_register_memory_map(paths, memory_map)?;
    let evidence = validate_register_evidence(paths, memory_map)?;
    let passed = !arguments.deny_unreviewed || summary.unreviewed == 0;
    if !passed {
        tracing::warn!(
            unreviewed = summary.unreviewed,
            "register workspace contains unreviewed entries"
        );
    }
    let report = RegisterWorkspaceDocument {
        schema: 1,
        command: "registers validate",
        status: if passed { "valid" } else { "unreviewed" },
        deny_unreviewed: arguments.deny_unreviewed,
        format: workspace.format_label(),
        ranges: summary.ranges,
        observed: summary.observed,
        reviewed: summary.reviewed,
        ignored: summary.ignored,
        manual: summary.manual,
        unreviewed: summary.unreviewed,
        fields: summary.fields,
        facts: &paths.facts,
        model: &paths.model,
        pac_api: api_pack.as_ref().map(|pack| PacApiDocument {
            schema: pack.schema,
            operations: pack.operation_count(),
            sources: pack.source_ids().len(),
            pack: paths
                .api_pack
                .as_deref()
                .expect("loaded API pack has a configured path"),
        }),
        lints: lint_pack.as_ref().map(|pack| RegisterLintDocument {
            schema: pack.schema,
            forbidden_field_name_substrings: pack.forbidden_field_name_substrings.len(),
            pack: paths
                .lint_pack
                .as_deref()
                .expect("validated lint pack has a configured path"),
        }),
        memory: memory.map(|summary| RegisterMemoryDocument {
            registers: summary.registers,
            mmio_regions: summary.mmio_regions,
        }),
        evidence: evidence.as_ref().map(|evidence| RegisterEvidenceDocument {
            catalogs: paths.evidence_catalogs.len(),
            confidence_levels: evidence.confidence_levels.len(),
            sources: evidence.sources.len(),
            ranges: evidence.ranges.len(),
        }),
    };
    crate::cli::output::render_report(
        &report,
        || print_workspace_human(&report),
        || print_workspace_tsv(&report),
    );
    Ok(passed)
}
