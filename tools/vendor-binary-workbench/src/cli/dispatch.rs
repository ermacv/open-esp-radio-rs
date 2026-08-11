//! Dispatch of fully resolved invocations into domain workflows.

use super::{commands, resolver::ResolvedInvocation};
use crate::Result;

pub(super) fn run(invocation: ResolvedInvocation) -> Result<bool> {
    match invocation {
        ResolvedInvocation::GenerateCompletions(arguments) => commands::run_completions(arguments),
        ResolvedInvocation::GenerateManpage(arguments) => commands::run_manpage(arguments),
        ResolvedInvocation::ProjectInit(arguments) => commands::run_project_init(arguments),
        ResolvedInvocation::ProjectConfigure {
            arguments,
            project_path,
        } => commands::run_project_configure(arguments, &project_path),
        ResolvedInvocation::ProjectInputsInit {
            arguments,
            project_path,
        } => commands::run_project_inputs_init(arguments, &project_path),
        ResolvedInvocation::ProjectBrowse { project_path } => {
            commands::run_project_browser(&project_path)
        }
        ResolvedInvocation::ProjectDoctor(session) => {
            commands::run_project_doctor(session.context())
        }
        ResolvedInvocation::ProjectFiles(session) => commands::run_project_files(session.context()),
        ResolvedInvocation::ProjectStatus { arguments, session } => {
            commands::run_project_status(arguments, session.context())
        }
        ResolvedInvocation::ProjectAnalyze { arguments, session } => {
            commands::run_project_analysis(arguments, &session)
        }
        ResolvedInvocation::ProjectVerify { arguments, session } => {
            commands::run_project_verification(
                arguments,
                &session.manifest,
                &session.project,
                session.run_spec.as_ref(),
                &session.mmio,
                &session.target,
            )
        }
        ResolvedInvocation::ProjectCheck { arguments, session } => {
            commands::run_project_check(arguments, &session)
        }
        ResolvedInvocation::ProjectPublish { arguments, session } => {
            commands::run_project_publication(
                arguments,
                &session.project,
                session.memory_map.as_ref(),
            )
        }
        ResolvedInvocation::FunctionWorkspace {
            command,
            project,
            target,
        } => commands::run_function_pack_command(command, &project, &target),
        ResolvedInvocation::CodeWorkspace { command, project } => {
            commands::run_code_workspace_command(command, &project)
        }
        ResolvedInvocation::RegisterWorkspace {
            command,
            project,
            memory_map,
        } => commands::run_register_command(command, &project, memory_map.as_ref()),
        ResolvedInvocation::InterfaceWorkspace {
            command,
            project,
            target,
        } => commands::run_interface_pack_command(command, &project, &target),
        ResolvedInvocation::SymbolInventory {
            arguments,
            run_spec,
        } => commands::run_symbol_inventory(arguments, &run_spec),
        ResolvedInvocation::InterfaceDiscover {
            arguments,
            run_spec,
            project,
        } => commands::run_interface_discovery(arguments, &run_spec, project.as_ref()),
        ResolvedInvocation::BuildIr {
            arguments,
            project,
            run_spec,
            target,
            svd,
        } => commands::run_ir_build(arguments, &project, &run_spec, &svd, &target),
        ResolvedInvocation::VerifyEvidence(arguments) => commands::run_verify_evidence(arguments),
        ResolvedInvocation::Target {
            command,
            target,
            svd,
            project,
        } => commands::run_target(command, &svd, &target, project.as_deref()),
    }
}
