//! Project interface-pack lifecycle commands.

use super::super::*;
use crate::{
    cli::resolver::InterfaceWorkspaceCommand,
    interfaces::{InterfaceFacts, InterfaceWorkspace, write_pack_template},
    project::ProjectSpec,
};

mod report;

use report::*;

pub(super) fn run(
    command: InterfaceWorkspaceCommand,
    project: &ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    let paths = project
        .interfaces
        .as_ref()
        .ok_or("project has no [interfaces] table; configure facts and pack paths first")
        .map_err(crate::Error::invalid)?;
    match command {
        InterfaceWorkspaceCommand::InitPack(arguments) => {
            init_pack(arguments, project, target, paths)
        }
        InterfaceWorkspaceCommand::Validate(arguments) => validate(arguments, target, paths),
    }
}

fn init_pack(
    arguments: OutputArgs,
    project: &ProjectSpec,
    target: &TargetSpec,
    paths: &crate::project::InterfaceWorkspacePaths,
) -> Result<bool> {
    let facts = InterfaceFacts::load(&paths.facts)?;
    let output = arguments
        .output
        .as_deref()
        .or(paths.pack.as_deref())
        .ok_or("interfaces init-pack requires [interfaces].pack or an explicit --output PATH")
        .map_err(crate::Error::invalid)?;
    write_pack_template(
        output,
        &facts,
        &project.id,
        target.calling_convention.label(),
    )?;
    let report = InterfacePackDocument {
        schema: 1,
        command: "interfaces init-pack",
        status: "created",
        tables: facts.tables.len(),
        observed_slots: facts.observed_slots(),
        observed_calls: facts.observed_calls(),
        path: output,
    };
    crate::cli::output::render_report(&report, || print_pack_human(&report));
    Ok(true)
}

fn validate(
    arguments: ValidationArgs,
    target: &TargetSpec,
    paths: &crate::project::InterfaceWorkspacePaths,
) -> Result<bool> {
    let pack = paths
        .pack
        .as_deref()
        .ok_or("interfaces validate requires [interfaces].pack")
        .map_err(crate::Error::invalid)?;
    let workspace = InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        target.calling_convention.label(),
        target
            .harness
            .as_deref()
            .map(crate::harnesses::contracts)
            .transpose()?,
    )?;
    let summary = workspace.summary();
    let passed = !arguments.deny_unreviewed
        || (summary.unreviewed_anchors == 0 && summary.unreviewed_slots == 0);
    let report = InterfaceWorkspaceDocument {
        schema: 1,
        command: "interfaces validate",
        status: if passed { "valid" } else { "unreviewed" },
        deny_unreviewed: arguments.deny_unreviewed,
        calling_convention: target.calling_convention.label(),
        fact_tables: summary.fact_tables,
        observed_slots: summary.observed_slots,
        observed_calls: summary.observed_calls,
        resolved_calls: summary.resolved_calls,
        reviewed_anchors: summary.reviewed_anchors,
        ignored_anchors: summary.ignored_anchors,
        unreviewed_anchors: summary.unreviewed_anchors,
        manual_anchors: summary.manual_anchors,
        reviewed_slots: summary.reviewed_slots,
        ignored_slots: summary.ignored_slots,
        unreviewed_slots: summary.unreviewed_slots,
        manual_slots: summary.manual_slots,
        semantic_links: summary.semantic_links,
        semantic_operations: summary.semantic_operations,
        artifact_guards: summary.artifact_guards,
        runtime_guards: summary.runtime_guards,
        execution_contracts: summary.execution_contracts,
        execution_models: summary.execution_models,
        facts: &paths.facts,
        pack,
        contracts: workspace
            .contracts()
            .iter()
            .map(|contract| InterfaceContractDocument {
                id: &contract.id,
                pack: &contract.pack,
                anchor: &contract.anchor,
                source: &contract.source,
                root_kind: interface_root_kind(&contract.root),
                container_depth: contract.container_path.len(),
                layout_version: &contract.layout_version,
                pointer_width: contract.pointer_width,
                layout_size: contract.layout_size,
                slot_stride: contract.slot_stride,
                guards: contract.guards.len(),
                execution_contract: contract
                    .execution_contract
                    .as_ref()
                    .map(|contract| contract.id.as_str()),
                slots: contract.slots.len(),
            })
            .collect(),
        bindings: workspace
            .bindings()
            .iter()
            .map(|binding| InterfaceBindingDocument {
                id: &binding.id,
                anchor: &binding.anchor,
                source: &binding.source,
                layout_version: &binding.layout_version,
                offset: binding.offset,
                width: binding.width,
                name: &binding.name,
                arguments: &binding.arguments,
                return_type: &binding.return_type,
                variadic: binding.variadic,
                semantic: binding
                    .semantic_annotation
                    .as_ref()
                    .map(|semantic| semantic.operation.as_str()),
                semantic_summary: binding
                    .semantic_annotation
                    .as_ref()
                    .map(|semantic| semantic.summary.as_str()),
                semantic_domain: binding
                    .semantic_annotation
                    .as_ref()
                    .map(|semantic| semantic.domain.as_str()),
                semantic_argument_roles: binding
                    .semantic_annotation
                    .as_ref()
                    .map_or_else(Vec::new, |semantic| {
                        semantic.argument_roles.iter().map(String::as_str).collect()
                    }),
                semantic_return_role: binding
                    .semantic_annotation
                    .as_ref()
                    .map(|semantic| semantic.return_role.as_str()),
                semantic_effects: binding
                    .semantic_annotation
                    .as_ref()
                    .map_or_else(Vec::new, |semantic| {
                        semantic.effects.iter().map(String::as_str).collect()
                    }),
                replacement: binding
                    .semantic_annotation
                    .as_ref()
                    .and_then(|semantic| semantic.replacement.as_deref()),
                execution_model: binding.execution_model.as_ref().map(|model| {
                    InterfaceExecutionModelDocument {
                        id: &model.id,
                        set: &model.set,
                        model: &model.model,
                        return_model: execution_model_label(model.return_model),
                    }
                }),
                functions: binding.functions.iter().map(String::as_str).collect(),
                calls: binding
                    .calls
                    .iter()
                    .map(|call| InterfaceCallDocument {
                        artifact: call.artifact,
                        member: call.member.as_deref(),
                        function: &call.function,
                        function_address: call.function_address,
                        site: call.site,
                        kind: &call.kind,
                        jalr_offset: call.jalr_offset,
                        slot_selector: call.slot_selector.as_deref(),
                        slot_index: call.slot_index,
                        slot_index_domain: call.slot_index_domain.as_ref().map(|domain| {
                            InterfaceIndexDomainDocument {
                                argument: domain.argument,
                                min: domain.min,
                                max: domain.max,
                                evidence: &domain.evidence,
                            }
                        }),
                        arguments: call
                            .arguments
                            .iter()
                            .map(|argument| InterfaceArgumentDocument {
                                index: argument.index,
                                kind: &argument.kind,
                                expression: &argument.expression,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
        unreviewed: workspace
            .unreviewed_observations()
            .iter()
            .map(|observation| UnreviewedInterfaceDocument {
                id: &observation.id,
                contract: &observation.contract,
                source: &observation.source,
                offset: observation.offset,
                width: observation.width,
                selector: observation.selector.as_deref(),
                functions: observation.functions.iter().map(String::as_str).collect(),
                call_sites: observation.call_sites.clone(),
            })
            .collect(),
    };
    crate::cli::output::render_report(&report, || print_workspace_human(&report));
    Ok(passed)
}

fn execution_model_label(model: crate::ExternalReturnModel) -> String {
    match model {
        crate::ExternalReturnModel::Constant(value) => format!("constant:{value:#010x}"),
        crate::ExternalReturnModel::SymbolicU32 => "symbolic-u32".to_owned(),
        crate::ExternalReturnModel::PrivateStackOutputU8 { pointer_argument } => {
            format!("private-stack-output-u8:a{pointer_argument}")
        }
        crate::ExternalReturnModel::Unmodeled => "unmodeled".to_owned(),
    }
}

const fn interface_root_kind(root: &crate::interfaces::InterfaceRootSelector) -> &'static str {
    match root {
        crate::interfaces::InterfaceRootSelector::RelocatedSymbol { .. } => "relocated-symbol",
        crate::interfaces::InterfaceRootSelector::FunctionArgument { .. } => "function-argument",
        crate::interfaces::InterfaceRootSelector::AbsoluteAddress { .. } => "absolute-address",
    }
}
