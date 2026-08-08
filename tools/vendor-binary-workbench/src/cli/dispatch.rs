//! Dispatch of fully resolved invocations into domain workflows.

use super::{
    args::Command,
    commands,
    resolver::{ResolvedCommandInvocation, ResolvedInvocation},
};
use crate::Result;

pub(super) fn run(invocation: ResolvedInvocation) -> Result<bool> {
    match invocation {
        ResolvedInvocation::ProjectInit { arguments } => commands::run_project_init(arguments),
        ResolvedInvocation::ProjectConfigure {
            arguments,
            project_path,
        } => commands::run_project_configure(arguments, &project_path),
        ResolvedInvocation::Command(invocation) => run_command(invocation),
    }
}

fn run_command(invocation: ResolvedCommandInvocation) -> Result<bool> {
    let ResolvedCommandInvocation {
        command,
        arguments,
        project_path,
        project,
        target_path,
        target,
        run_spec_path,
        run_spec,
        memory_map,
        svd_paths,
        svd,
    } = invocation;

    if matches!(command, Command::ProjectDoctor | Command::ProjectStatus) {
        let context = commands::ProjectContext {
            project_path: project_path
                .as_deref()
                .expect("project inspection requires a manifest path"),
            project: project
                .as_ref()
                .expect("project inspection requires a loaded project"),
            target_path: &target_path,
            target: &target,
            run_spec_path: run_spec_path.as_deref(),
            run_spec: run_spec.as_ref(),
            memory_map: memory_map.as_ref(),
            svd_paths: &svd_paths,
            svd: &svd,
        };
        return if command == Command::ProjectDoctor {
            commands::run_project_doctor(arguments, context)
        } else {
            commands::run_project_status(arguments, context)
        };
    }
    if command == Command::ProjectAnalyze {
        return commands::run_project_analysis(
            arguments,
            project
                .as_ref()
                .expect("project analysis requires a loaded project"),
            run_spec.as_ref(),
            memory_map.as_ref(),
            &svd,
            &target,
        );
    }
    if command == Command::ProjectPublish {
        return commands::run_project_publication(
            arguments,
            project
                .as_ref()
                .expect("project publication requires a loaded project"),
            memory_map.as_ref(),
        );
    }
    if matches!(
        command,
        Command::FunctionInitPack | Command::FunctionValidate | Command::FunctionReview
    ) {
        return commands::run_function_pack_command(
            command,
            arguments,
            project
                .as_ref()
                .expect("function pack commands require a loaded project"),
            &target,
        );
    }
    if command == Command::SymbolInventory {
        let run_spec = run_spec
            .as_ref()
            .ok_or("symbols inventory requires a run spec with artifact bindings")?;
        return commands::run_symbol_inventory(arguments, run_spec);
    }
    if command == Command::InterfaceDiscover {
        let run_spec = run_spec
            .as_ref()
            .ok_or("interfaces discover requires a run spec with artifact bindings")?;
        return commands::run_interface_discovery(arguments, run_spec);
    }
    if matches!(
        command,
        Command::InterfaceInitPack | Command::InterfaceValidate
    ) {
        return commands::run_interface_pack_command(
            command,
            arguments,
            project
                .as_ref()
                .expect("interface pack commands require a loaded project"),
            &target,
        );
    }
    if matches!(
        command,
        Command::RegisterInitModel
            | Command::RegisterImportSvd
            | Command::RegisterValidate
            | Command::RegisterReview
            | Command::RegisterExportSvd
            | Command::RegisterGeneratePac
            | Command::RegisterGenerateBindings
    ) {
        return commands::run_register_command(
            command,
            arguments,
            project
                .as_ref()
                .expect("register commands require a loaded project"),
            memory_map.as_ref(),
        );
    }
    if command == Command::BuildIr {
        return commands::run_ir_build(
            arguments,
            project
                .as_ref()
                .expect("project IR build requires a loaded project"),
            run_spec
                .as_ref()
                .ok_or("ir build requires a run spec with source artifact bindings")?,
            &svd,
            &target,
        );
    }
    commands::run(command, arguments, &svd, &target)
}
