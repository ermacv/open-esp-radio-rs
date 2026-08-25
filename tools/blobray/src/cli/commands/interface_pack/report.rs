//! Typed interface-workspace lifecycle reports and presentation renderers.

use std::path::Path;

use serde::Serialize;

const HUMAN_BINDING_LIMIT: usize = 64;
const HUMAN_CALL_LIMIT: usize = 32;
const HUMAN_ARGUMENT_LIMIT: usize = 4;
const HUMAN_EXPRESSION_LIMIT: usize = 48;

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
    pub(super) asserted_anchors: usize,
    pub(super) reviewed_slots: usize,
    pub(super) ignored_slots: usize,
    pub(super) unreviewed_slots: usize,
    pub(super) asserted_slots: usize,
    pub(super) semantic_links: usize,
    pub(super) semantic_operations: usize,
    pub(super) artifact_guards: usize,
    pub(super) runtime_guards: usize,
    pub(super) execution_contracts: usize,
    pub(super) execution_models: usize,
    pub(super) capability_packs: usize,
    pub(super) interface_template_packs: usize,
    pub(super) interface_templates: usize,
    pub(super) templated_anchors: usize,
    pub(super) template_pack_ids: &'a [String],
    pub(super) templates: Vec<InterfaceTemplateDocument<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) capabilities: Option<crate::interfaces::CapabilityEvaluationReport>,
    pub(super) facts: &'a Path,
    pub(super) pack: &'a Path,
    pub(super) contracts: Vec<InterfaceContractDocument<'a>>,
    pub(super) bindings: Vec<InterfaceBindingDocument<'a>>,
    pub(super) unreviewed: Vec<UnreviewedInterfaceDocument<'a>>,
}

#[derive(Serialize)]
pub(super) struct UnreviewedInterfaceDocument<'a> {
    pub(super) id: &'a str,
    pub(super) contract: &'a str,
    pub(super) source: &'a str,
    pub(super) offset: i32,
    pub(super) width: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) selector: Option<&'a str>,
    pub(super) functions: Vec<&'a str>,
    pub(super) call_sites: Vec<u32>,
}

#[derive(Serialize)]
pub(super) struct InterfaceContractDocument<'a> {
    pub(super) id: &'a str,
    pub(super) pack: &'a str,
    pub(super) anchor: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) template: Option<&'a str>,
    pub(super) template_overrides: Vec<InterfaceTemplateOverrideDocument<'a>>,
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
    pub(super) slots: usize,
}

#[derive(Serialize)]
pub(super) struct InterfaceTemplateDocument<'a> {
    pub(super) id: &'a str,
    pub(super) repository: &'a str,
    pub(super) revision: &'a str,
    pub(super) path: &'a str,
}

#[derive(Serialize)]
pub(super) struct InterfaceTemplateOverrideDocument<'a> {
    pub(super) offset: i32,
    pub(super) reason: &'a str,
    pub(super) fields: &'a [String],
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
    pub(super) set: &'a str,
    pub(super) model: &'a str,
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
                "Reviewed assertion"
            ],
            [
                [
                    "Anchors".into(),
                    report.fact_tables.to_string(),
                    report.reviewed_anchors.to_string(),
                    report.ignored_anchors.to_string(),
                    report.unreviewed_anchors.to_string(),
                    report.asserted_anchors.to_string(),
                ],
                [
                    "Slots".into(),
                    report.observed_slots.to_string(),
                    report.reviewed_slots.to_string(),
                    report.ignored_slots.to_string(),
                    report.unreviewed_slots.to_string(),
                    report.asserted_slots.to_string(),
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
                        "{} ptr={} stride={} template={} overrides={}",
                        contract.layout_version,
                        contract.pointer_width,
                        contract.slot_stride,
                        contract.template.unwrap_or("-"),
                        contract.template_overrides.len(),
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
                report
                    .bindings
                    .iter()
                    .take(HUMAN_BINDING_LIMIT)
                    .map(|binding| [
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
        if report.bindings.len() > HUMAN_BINDING_LIMIT {
            outputln!(
                "Bindings: {} more omitted; use `--format json` for the complete result",
                report.bindings.len() - HUMAN_BINDING_LIMIT
            );
        }
        let calls = report
            .bindings
            .iter()
            .flat_map(|binding| {
                binding.calls.iter().map(move |call| {
                    let mut arguments = call
                        .arguments
                        .iter()
                        .take(HUMAN_ARGUMENT_LIMIT)
                        .map(|argument| {
                            format!(
                                "a{}:{}={}",
                                argument.index,
                                argument.kind,
                                compact_text(argument.expression, HUMAN_EXPRESSION_LIMIT)
                            )
                        })
                        .collect::<Vec<_>>();
                    if call.arguments.len() > HUMAN_ARGUMENT_LIMIT {
                        arguments.push(format!(
                            "+{} arg(s)",
                            call.arguments.len() - HUMAN_ARGUMENT_LIMIT
                        ));
                    }
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
                        arguments.join(", "),
                    ]
                })
            })
            .take(HUMAN_CALL_LIMIT)
            .collect::<Vec<_>>();
        if !calls.is_empty() {
            outputln!(
                "Resolved calls:\n{}",
                crate::cli::table::render(
                    ["Anchor", "Function", "Site", "Kind", "Arguments"],
                    calls,
                )
            );
            if report.resolved_calls > HUMAN_CALL_LIMIT {
                outputln!(
                    "Resolved calls: {} more omitted; use `--format json` for exact call sites and arguments",
                    report.resolved_calls - HUMAN_CALL_LIMIT
                );
            }
        }
    }
    outputln!(
        "Summary: observed-calls={} resolved-calls={} semantic-links={} operations={} execution-contracts={} execution-models={} capability-packs={} interface-template-packs={} templates={} templated-anchors={} artifact-guards={} runtime-guards={}",
        report.observed_calls,
        report.resolved_calls,
        report.semantic_links,
        report.semantic_operations,
        report.execution_contracts,
        report.execution_models,
        report.capability_packs,
        report.interface_template_packs,
        report.interface_templates,
        report.templated_anchors,
        report.artifact_guards,
        report.runtime_guards,
    );
    if let Some(capabilities) = &report.capabilities {
        outputln!(
            "Reusable capability rules: {}\n{}",
            capabilities.status.label(),
            crate::cli::table::render(
                ["Rule", "Protocol", "Scope", "Status", "Matches"],
                capabilities.rules.iter().map(|rule| [
                    rule.id.clone(),
                    rule.protocol.clone(),
                    rule.scope.clone(),
                    rule.status.label().to_owned(),
                    rule.requirements
                        .iter()
                        .map(|requirement| requirement.matches.len())
                        .sum::<usize>()
                        .to_string(),
                ]),
            )
        );
    }
    if !report.unreviewed.is_empty() {
        outputln!(
            "Unreviewed observations: {} (use `--format json` for exact selectors, functions, and call sites)",
            report.unreviewed.len()
        );
    }
}

fn compact_text(value: &str, limit: usize) -> String {
    let mut characters = value.chars();
    let prefix = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::compact_text;

    #[test]
    fn human_expression_preview_is_bounded_on_character_boundaries() {
        assert_eq!(compact_text("short", 8), "short");
        assert_eq!(compact_text("абвгд", 3), "абв…");
    }
}
