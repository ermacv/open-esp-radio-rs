//! Typed interface-workspace lifecycle reports and presentation renderers.

use std::path::Path;

use serde::Serialize;

#[derive(Serialize)]
pub(super) struct InterfacePackDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) tables: usize,
    pub(super) observed_slots: usize,
    pub(super) observed_calls: usize,
    pub(super) path: &'a Path,
}

#[derive(Serialize)]
pub(super) struct InterfacePackSyncDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) check: bool,
    pub(super) added_anchors: usize,
    pub(super) refreshed_anchors: usize,
    pub(super) removed_anchors: usize,
    pub(super) added_slots: usize,
    pub(super) removed_slots: usize,
    pub(super) facts: &'a Path,
    pub(super) pack: &'a Path,
}

#[derive(Serialize)]
pub(super) struct InterfaceWorkspaceDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) deny_unreviewed: bool,
    pub(super) calling_convention: &'a str,
    pub(super) fact_tables: usize,
    pub(super) observed_slots: usize,
    pub(super) observed_calls: usize,
    pub(super) resolved_calls: usize,
    pub(super) reviewed_anchors: usize,
    pub(super) ignored_anchors: usize,
    pub(super) unreviewed_anchors: usize,
    pub(super) manual_anchors: usize,
    pub(super) reviewed_slots: usize,
    pub(super) ignored_slots: usize,
    pub(super) unreviewed_slots: usize,
    pub(super) manual_slots: usize,
    pub(super) semantic_links: usize,
    pub(super) semantic_operations: usize,
    pub(super) artifact_guards: usize,
    pub(super) runtime_guards: usize,
    pub(super) execution_contracts: usize,
    pub(super) execution_models: usize,
    pub(super) facts: &'a Path,
    pub(super) pack: &'a Path,
    pub(super) contracts: Vec<InterfaceContractDocument<'a>>,
    pub(super) bindings: Vec<InterfaceBindingDocument<'a>>,
}

#[derive(Serialize)]
pub(super) struct InterfaceContractDocument<'a> {
    pub(super) id: &'a str,
    pub(super) pack: &'a str,
    pub(super) anchor: &'a str,
    pub(super) source: &'a str,
    pub(super) root_kind: &'static str,
    pub(super) container_depth: usize,
    pub(super) layout_version: &'a str,
    pub(super) pointer_width: u8,
    pub(super) layout_size: u32,
    pub(super) slot_stride: u8,
    pub(super) guards: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_contract: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_pointer_symbol: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_backing_symbol: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_magic: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_magic_offset: Option<u32>,
    pub(super) slots: usize,
}

#[derive(Serialize)]
pub(super) struct InterfaceBindingDocument<'a> {
    pub(super) id: &'a str,
    pub(super) anchor: &'a str,
    pub(super) source: &'a str,
    pub(super) layout_version: &'a str,
    pub(super) offset: i32,
    pub(super) width: u8,
    pub(super) name: &'a str,
    pub(super) arguments: &'a [String],
    pub(super) return_type: &'a str,
    pub(super) variadic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) semantic: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) semantic_summary: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) semantic_domain: Option<&'a str>,
    pub(super) semantic_argument_roles: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) semantic_return_role: Option<&'a str>,
    pub(super) semantic_effects: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) replacement: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) execution_model: Option<InterfaceExecutionModelDocument<'a>>,
    pub(super) functions: Vec<&'a str>,
    pub(super) calls: Vec<InterfaceCallDocument<'a>>,
}

#[derive(Serialize)]
pub(super) struct InterfaceExecutionModelDocument<'a> {
    pub(super) id: &'a str,
    pub(super) table: &'a str,
    pub(super) function: &'a str,
    pub(super) c_name: &'a str,
    pub(super) return_model: String,
}

#[derive(Serialize)]
pub(super) struct InterfaceCallDocument<'a> {
    pub(super) artifact: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) member: Option<&'a str>,
    pub(super) function: &'a str,
    pub(super) function_address: u32,
    pub(super) site: u32,
    pub(super) kind: &'a str,
    pub(super) jalr_offset: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) slot_selector: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) slot_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) slot_index_domain: Option<InterfaceIndexDomainDocument<'a>>,
    pub(super) arguments: Vec<InterfaceArgumentDocument<'a>>,
}

#[derive(Serialize)]
pub(super) struct InterfaceIndexDomainDocument<'a> {
    pub(super) argument: u8,
    pub(super) min: u32,
    pub(super) max: u32,
    pub(super) evidence: &'a str,
}

#[derive(Serialize)]
pub(super) struct InterfaceArgumentDocument<'a> {
    pub(super) index: usize,
    pub(super) kind: &'a str,
    pub(super) expression: &'a str,
}

pub(super) fn print_pack_human(report: &InterfacePackDocument<'_>) {
    outputln!(
        "Interface pack: {} — {}",
        report.status,
        report.path.display()
    );
    outputln!(
        "{}",
        crate::cli::table::render(
            ["Tables", "Observed slots", "Observed calls"],
            [[
                report.tables.to_string(),
                report.observed_slots.to_string(),
                report.observed_calls.to_string(),
            ]],
        )
    );
}

pub(super) fn print_sync_human(report: &InterfacePackSyncDocument<'_>) {
    outputln!(
        "Interface pack synchronization: {} — {}",
        report.status,
        report.pack.display()
    );
    outputln!(
        "{}",
        crate::cli::table::render(
            ["Scope", "Added", "Refreshed", "Removed"],
            [
                [
                    "Anchors".to_owned(),
                    report.added_anchors.to_string(),
                    report.refreshed_anchors.to_string(),
                    report.removed_anchors.to_string(),
                ],
                [
                    "Slots".to_owned(),
                    report.added_slots.to_string(),
                    "-".to_owned(),
                    report.removed_slots.to_string(),
                ],
            ],
        )
    );
}

pub(super) fn print_workspace_human(report: &InterfaceWorkspaceDocument<'_>) {
    outputln!(
        "Interface workspace: {} — {}",
        report.status,
        report.pack.display()
    );
    outputln!(
        "Coverage:\n{}",
        crate::cli::table::render(
            [
                "Scope",
                "Observed",
                "Reviewed",
                "Ignored",
                "Unreviewed",
                "Manual"
            ],
            [
                [
                    "Anchors".into(),
                    report.fact_tables.to_string(),
                    report.reviewed_anchors.to_string(),
                    report.ignored_anchors.to_string(),
                    report.unreviewed_anchors.to_string(),
                    report.manual_anchors.to_string(),
                ],
                [
                    "Slots".into(),
                    report.observed_slots.to_string(),
                    report.reviewed_slots.to_string(),
                    report.ignored_slots.to_string(),
                    report.unreviewed_slots.to_string(),
                    report.manual_slots.to_string(),
                ],
            ],
        )
    );
    if !report.contracts.is_empty() {
        outputln!(
            "Resolved contracts:\n{}",
            crate::cli::table::render(
                [
                    "Contract",
                    "Source",
                    "Root",
                    "Layout",
                    "Size",
                    "Slots",
                    "Execution contract",
                ],
                report.contracts.iter().map(|contract| [
                    contract.id.to_owned(),
                    contract.source.to_owned(),
                    format!("{} depth={}", contract.root_kind, contract.container_depth),
                    format!(
                        "{} ptr={} stride={}",
                        contract.layout_version, contract.pointer_width, contract.slot_stride
                    ),
                    format!("{:#x}", contract.layout_size),
                    contract.slots.to_string(),
                    contract.execution_contract.unwrap_or("-").to_owned(),
                ]),
            )
        );
    }
    if !report.bindings.is_empty() {
        outputln!(
            "Bindings:\n{}",
            crate::cli::table::render(
                [
                    "Slot",
                    "Source",
                    "Layout",
                    "Offset",
                    "Width",
                    "Name",
                    "ABI",
                    "Semantic",
                    "Execution",
                    "Calls",
                ],
                report.bindings.iter().map(|binding| [
                    binding.id.to_owned(),
                    binding.source.to_owned(),
                    binding.layout_version.to_owned(),
                    format!("{:+#x}", binding.offset),
                    binding.width.to_string(),
                    binding.name.to_owned(),
                    format!(
                        "{}({})->{}{}",
                        report.calling_convention,
                        binding.arguments.join(", "),
                        binding.return_type,
                        if binding.variadic { ", ..." } else { "" },
                    ),
                    binding.semantic.unwrap_or("-").to_owned(),
                    binding
                        .execution_model
                        .as_ref()
                        .map_or("-", |model| model.id)
                        .to_owned(),
                    binding.calls.len().to_string(),
                ]),
            )
        );
        let calls = report
            .bindings
            .iter()
            .flat_map(|binding| {
                binding.calls.iter().map(move |call| {
                    [
                        binding.anchor.to_owned(),
                        call.function.to_owned(),
                        format!("{:#010x}", call.site),
                        call.slot_selector.map_or_else(
                            || call.kind.to_owned(),
                            |selector| {
                                format!(
                                    "{} indexed({selector}) index={} domain={}",
                                    call.kind,
                                    call.slot_index
                                        .map_or_else(|| "?".to_owned(), |index| index.to_string()),
                                    call.slot_index_domain.as_ref().map_or_else(
                                        || "?".to_owned(),
                                        |domain| {
                                            format!(
                                                "arg{}:{}..={}",
                                                domain.argument, domain.min, domain.max
                                            )
                                        }
                                    ),
                                )
                            },
                        ),
                        call.arguments
                            .iter()
                            .map(|argument| {
                                format!(
                                    "a{}:{}={}",
                                    argument.index, argument.kind, argument.expression
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                })
            })
            .collect::<Vec<_>>();
        if !calls.is_empty() {
            outputln!(
                "Resolved calls:\n{}",
                crate::cli::table::render(
                    ["Anchor", "Function", "Site", "Kind", "Arguments"],
                    calls,
                )
            );
        }
    }
    outputln!(
        "Summary: observed-calls={} resolved-calls={} semantic-links={} operations={} execution-contracts={} execution-models={} artifact-guards={} runtime-guards={}",
        report.observed_calls,
        report.resolved_calls,
        report.semantic_links,
        report.semantic_operations,
        report.execution_contracts,
        report.execution_models,
        report.artifact_guards,
        report.runtime_guards,
    );
}
