//! Project interface-pack lifecycle commands.

use super::super::*;
use crate::{
    interfaces::{InterfaceFacts, InterfaceWorkspace, write_pack_template},
    project::ProjectSpec,
};

mod report;

use report::*;

pub(super) fn run(
    command: Command,
    arguments: CommandArguments,
    project: &ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    let paths = project
        .interfaces
        .as_ref()
        .ok_or("project has no [interfaces] table; configure facts and pack paths first")
        .map_err(crate::Error::invalid)?;
    match (command, arguments) {
        (Command::InterfaceInitPack, CommandArguments::Output(arguments)) => {
            init_pack(arguments, project, target, paths)
        }
        (Command::InterfaceValidate, CommandArguments::Validation(arguments)) => {
            validate(arguments, target, paths)
        }
        _ => unreachable!("interface pack dispatcher received another command"),
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
    crate::cli::output::render_report(
        &report,
        || print_pack_human(&report),
        || print_pack_tsv(&report),
    );
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
        facts: &paths.facts,
        pack,
        bindings: workspace
            .bindings()
            .iter()
            .map(|binding| InterfaceBindingDocument {
                anchor: &binding.anchor,
                source: &binding.source,
                layout_version: &binding.layout_version,
                offset: binding.offset,
                width: binding.width,
                name: &binding.name,
                arguments: &binding.arguments,
                return_type: &binding.return_type,
                variadic: binding.variadic,
                semantic: binding.semantic.as_deref(),
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
    };
    crate::cli::output::render_report(
        &report,
        || print_workspace_human(&report),
        || print_workspace_tsv(&report),
    );
    Ok(passed)
}
