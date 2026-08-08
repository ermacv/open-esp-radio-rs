//! Project interface-pack lifecycle commands.

use super::super::*;
use crate::{
    interfaces::{InterfaceFacts, InterfaceWorkspace, write_pack_template},
    project::ProjectSpec,
};

pub(super) fn run(
    command: Command,
    arguments: CommandArguments,
    project: &ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    let paths = project
        .interfaces
        .as_ref()
        .ok_or("project has no [interfaces] table; configure facts and pack paths first")?;
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
        .ok_or("interfaces init-pack requires [interfaces].pack or an explicit --output PATH")?;
    write_pack_template(
        output,
        &facts,
        &project.id,
        target.calling_convention.label(),
    )?;
    outputln!(
        "INTERFACE-PACK\tstatus=created\ttables={}\tobserved-slots={}\tobserved-calls={}\tpath={}",
        facts.tables.len(),
        facts.observed_slots(),
        facts.observed_calls(),
        output.display()
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
        .ok_or("interfaces validate requires [interfaces].pack")?;
    let workspace = InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        target.calling_convention.label(),
    )?;
    let summary = workspace.summary();
    for binding in workspace.bindings() {
        outputln!(
            "INTERFACE-BINDING\tanchor={}\tsource={}\tlayout-version={}\toffset={:+#x}\twidth={}\tname={}\tabi={}({})->{}{}\tsemantic={}\tfunctions={}\tcall-sites={}",
            binding.anchor,
            binding.source,
            binding.layout_version,
            binding.offset,
            binding.width,
            binding.name,
            target.calling_convention.label(),
            binding.arguments.join(","),
            binding.return_type,
            if binding.variadic { ",..." } else { "" },
            binding.semantic.as_deref().unwrap_or("-"),
            binding
                .functions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
            binding.calls.len(),
        );
        for call in &binding.calls {
            outputln!(
                "INTERFACE-CALL\tanchor={}\tsource={}\toffset={:+#x}\tartifact={}\tmember={}\tfunction={}\tfunction-address={:#010x}\tsite={:#010x}\tkind={}\tjalr-offset={:+#x}\targuments={}",
                binding.anchor,
                binding.source,
                binding.offset,
                call.artifact,
                call.member.as_deref().unwrap_or("-"),
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
                    .join(","),
            );
        }
    }
    outputln!(
        "INTERFACE-WORKSPACE\tstatus=valid\tfact-tables={}\tobserved-slots={}\tobserved-calls={}\tresolved-calls={}\treviewed-anchors={}\tignored-anchors={}\tunreviewed-anchors={}\tmanual-anchors={}\treviewed-slots={}\tignored-slots={}\tunreviewed-slots={}\tmanual-slots={}\tsemantic-links={}\tsemantic-operations={}\tartifact-guards={}\truntime-guards={}\tfacts={}\tpack={}",
        summary.fact_tables,
        summary.observed_slots,
        summary.observed_calls,
        summary.resolved_calls,
        summary.reviewed_anchors,
        summary.ignored_anchors,
        summary.unreviewed_anchors,
        summary.manual_anchors,
        summary.reviewed_slots,
        summary.ignored_slots,
        summary.unreviewed_slots,
        summary.manual_slots,
        summary.semantic_links,
        summary.semantic_operations,
        summary.artifact_guards,
        summary.runtime_guards,
        paths.facts.display(),
        pack.display(),
    );
    Ok(!arguments.deny_unreviewed
        || (summary.unreviewed_anchors == 0 && summary.unreviewed_slots == 0))
}
