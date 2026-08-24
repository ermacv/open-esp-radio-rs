//! Project register-workspace lifecycle commands.

use super::super::*;
use crate::{cli::resolver::RegisterWorkspaceCommand, project::ProjectSpec, registers::*};

mod publication;
mod report;

use publication::{export_svd, generate_bindings, generate_pac_raw_source};
use report::*;

pub(super) fn run(
    command: RegisterWorkspaceCommand,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or("project has no [registers] table; configure facts and model paths first")
        .map_err(crate::Error::invalid)?;
    match command {
        RegisterWorkspaceCommand::InitModel(arguments) => {
            init_model(arguments, project, memory_map, paths)
        }
        RegisterWorkspaceCommand::ImportSvd(arguments) => import_svd(arguments, memory_map, paths),
        RegisterWorkspaceCommand::Validate(arguments) => validate(arguments, memory_map, paths),
        RegisterWorkspaceCommand::Review(arguments) => review(arguments, paths),
        RegisterWorkspaceCommand::ExportSvd(arguments) => export_svd(arguments, paths),
        RegisterWorkspaceCommand::GeneratePacRaw(arguments) => {
            generate_pac_raw_source(arguments, paths)
        }
        RegisterWorkspaceCommand::GenerateBindings(arguments) => {
            generate_bindings(arguments, paths)
        }
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
    let (contents, summary) = render_register_review(
        &facts,
        &model,
        &ir_reports,
        &paths.owned_ranges,
        &paths.non_operational_functions,
        &paths.facts,
        &paths.model,
    )?;
    crate::application::generated_file::write_or_check(
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
        ignored: summary.ignored,
        non_operational: summary.non_operational,
        unreviewed: summary.unreviewed,
        model_only: summary.model_only,
        draft_field_partitions: summary.field_candidates,
        ir_reports: summary.ir_reports,
        ir_registers: summary.ir_registers,
        ir_only_registers: summary.ir_only_registers,
        ir_field_candidates: summary.ir_field_candidates,
        path: output,
    };
    crate::cli::output::render_report(&report, || print_review_human(&report));
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
    let owned_facts = facts.select_ranges(&paths.owned_ranges)?;
    let summary = init_register_model(&owned_facts, output, &address_space, &project.id)?;
    let report = RegisterModelDocument {
        schema: 1,
        command: "registers init-model",
        status: "created",
        model_schema: 2,
        peripherals: summary.peripherals,
        fragments: summary.fragments,
        observed_registers: owned_facts.registers.len(),
        annotations: None,
        address_space: &address_space,
        input: None,
        model: output,
    };
    crate::cli::output::render_report(&report, || print_model_human(&report));
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
    crate::cli::output::render_report(&report, || print_model_human(&report));
    Ok(true)
}

fn validate(
    arguments: ValidationArgs,
    memory_map: Option<&MemoryMap>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let workspace = ProjectRegisterWorkspace::load(paths)?;
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
        non_operational: summary.non_operational,
        manual: summary.manual,
        unreviewed: summary.unreviewed,
        fields: summary.fields,
        facts: &paths.facts,
        model: &paths.model,
        pac_api: api_pack.as_ref().map(|pack| PacApiDocument {
            schema: pack.schema,
            ownership_partitions: pack.ownership_partition_count(),
            domains: pack.domain_count(),
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
            sources: evidence.sources.len(),
            ranges: evidence.ranges.len(),
        }),
    };
    crate::cli::output::render_report(&report, || print_workspace_human(&report));
    Ok(passed)
}
