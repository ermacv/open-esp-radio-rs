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
    pub(super) facts: &'a Path,
    pub(super) pack: &'a Path,
    pub(super) bindings: Vec<InterfaceBindingDocument<'a>>,
}

#[derive(Serialize)]
pub(super) struct InterfaceBindingDocument<'a> {
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
    pub(super) functions: Vec<&'a str>,
    pub(super) calls: Vec<InterfaceCallDocument<'a>>,
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
    pub(super) arguments: Vec<InterfaceArgumentDocument<'a>>,
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

pub(super) fn print_pack_tsv(report: &InterfacePackDocument<'_>) {
    outputln!(
        "INTERFACE-PACK\tstatus={}\ttables={}\tobserved-slots={}\tobserved-calls={}\tpath={}",
        report.status,
        report.tables,
        report.observed_slots,
        report.observed_calls,
        report.path.display()
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
    if !report.bindings.is_empty() {
        outputln!(
            "Bindings:\n{}",
            crate::cli::table::render(
                [
                    "Anchor", "Source", "Layout", "Offset", "Width", "Name", "ABI", "Semantic",
                    "Calls",
                ],
                report.bindings.iter().map(|binding| [
                    binding.anchor.to_owned(),
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
                        call.kind.to_owned(),
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
        "Summary: observed-calls={} resolved-calls={} semantic-links={} operations={} artifact-guards={} runtime-guards={}",
        report.observed_calls,
        report.resolved_calls,
        report.semantic_links,
        report.semantic_operations,
        report.artifact_guards,
        report.runtime_guards,
    );
}

pub(super) fn print_workspace_tsv(report: &InterfaceWorkspaceDocument<'_>) {
    for binding in &report.bindings {
        outputln!(
            "INTERFACE-BINDING\tanchor={}\tsource={}\tlayout-version={}\toffset={:+#x}\twidth={}\tname={}\tabi={}({})->{}{}\tsemantic={}\tfunctions={}\tcall-sites={}",
            binding.anchor,
            binding.source,
            binding.layout_version,
            binding.offset,
            binding.width,
            binding.name,
            report.calling_convention,
            binding.arguments.join(","),
            binding.return_type,
            if binding.variadic { ",..." } else { "" },
            binding.semantic.unwrap_or("-"),
            binding.functions.join(","),
            binding.calls.len()
        );
        for call in &binding.calls {
            outputln!(
                "INTERFACE-CALL\tanchor={}\tsource={}\toffset={:+#x}\tartifact={}\tmember={}\tfunction={}\tfunction-address={:#010x}\tsite={:#010x}\tkind={}\tjalr-offset={:+#x}\targuments={}",
                binding.anchor,
                binding.source,
                binding.offset,
                call.artifact,
                call.member.unwrap_or("-"),
                call.function,
                call.function_address,
                call.site,
                call.kind,
                call.jalr_offset,
                call.arguments
                    .iter()
                    .map(|argument| format!(
                        "a{}:{}={}",
                        argument.index, argument.kind, argument.expression
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    outputln!(
        "INTERFACE-WORKSPACE\tstatus={}\tdeny-unreviewed={}\tfact-tables={}\tobserved-slots={}\tobserved-calls={}\tresolved-calls={}\treviewed-anchors={}\tignored-anchors={}\tunreviewed-anchors={}\tmanual-anchors={}\treviewed-slots={}\tignored-slots={}\tunreviewed-slots={}\tmanual-slots={}\tsemantic-links={}\tsemantic-operations={}\tartifact-guards={}\truntime-guards={}\tfacts={}\tpack={}",
        report.status,
        report.deny_unreviewed,
        report.fact_tables,
        report.observed_slots,
        report.observed_calls,
        report.resolved_calls,
        report.reviewed_anchors,
        report.ignored_anchors,
        report.unreviewed_anchors,
        report.manual_anchors,
        report.reviewed_slots,
        report.ignored_slots,
        report.unreviewed_slots,
        report.manual_slots,
        report.semantic_links,
        report.semantic_operations,
        report.artifact_guards,
        report.runtime_guards,
        report.facts.display(),
        report.pack.display()
    );
}
