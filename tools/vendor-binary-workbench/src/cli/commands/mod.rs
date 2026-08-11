//! Per-command parsing and execution.

mod audit_image_targets;
mod code_workspace;
mod discover_mmio;
mod execute_compare;
mod execute_run;
mod export_ir;
mod function_pack;
mod generate_driver;
mod generate_reference;
mod generate_reference_batch;
mod inspect_analyze;
mod inspect_compare;
mod inspect_flow;
mod inspect_function;
mod inspect_object;
mod inspect_scope;
mod inspect_trace;
mod interface_discovery;
mod interface_discovery_options;
mod interface_pack;
mod ir_build;
mod project_check;
mod project_configure;
mod project_doctor;
mod project_files;
mod project_function_doctor;
mod project_init;
mod project_inputs;
mod project_ir_doctor;
mod project_pipeline;
mod project_publication;
pub(crate) mod project_status;
mod project_verification;
mod registers;
mod symbol_inventory;
mod tooling;
mod verify_contract;
mod verify_evidence;
mod verify_inventory;
mod verify_profiles;
mod verify_source;

use super::{
    MmioMap, Result, TargetSpec,
    args::{CompletionArgs, ManpageArgs},
    resolver::{
        CodeWorkspaceCommand, FunctionWorkspaceCommand, InterfaceWorkspaceCommand,
        RegisterWorkspaceCommand, TargetCommand,
    },
};
use crate::application::ProjectContext;

pub(super) fn run_completions(arguments: CompletionArgs) -> Result<bool> {
    tooling::run_completions(arguments)
}

pub(super) fn run_manpage(arguments: ManpageArgs) -> Result<bool> {
    tooling::run_manpage(arguments)
}

pub(super) fn run_project_init(arguments: super::ProjectInitArgs) -> Result<bool> {
    project_init::run(arguments)
}

pub(super) fn run_project_configure(
    arguments: super::ProjectConfigureArgs,
    manifest: &std::path::Path,
) -> Result<bool> {
    project_configure::run(arguments, manifest)
}

pub(super) fn run_project_inputs_init(
    arguments: super::ProjectInputsInitArgs,
    manifest: &std::path::Path,
) -> Result<bool> {
    project_inputs::run(arguments, manifest)
}

pub(super) fn run_project_doctor(context: ProjectContext<'_>) -> Result<bool> {
    project_doctor::run(context)
}

pub(super) fn run_project_files(context: ProjectContext<'_>) -> Result<bool> {
    project_files::run(context)
}

pub(super) fn run_project_status(
    arguments: super::ProjectStatusArgs,
    context: ProjectContext<'_>,
) -> Result<bool> {
    project_status::run(arguments, context)
}

pub(super) fn run_project_browser(manifest: &std::path::Path) -> Result<bool> {
    crate::tui::run(manifest)
}

pub(super) fn run_project_analysis(
    arguments: super::ProjectAnalyzeArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    project_pipeline::run(arguments, session)
}

pub(super) fn run_project_publication(
    arguments: super::CheckArgs,
    project: &crate::project::ProjectSpec,
    memory_map: Option<&crate::MemoryMap>,
) -> Result<bool> {
    project_publication::run(arguments, project, memory_map)
}

pub(super) fn run_project_verification(
    arguments: super::ProjectVerifyArgs,
    project_manifest: &std::path::Path,
    project: &crate::project::ProjectSpec,
    run_spec: Option<&crate::run_spec::RunSpec>,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    project_verification::run(arguments, project_manifest, project, run_spec, svd, target)
}

pub(super) fn run_project_check(
    arguments: super::ProjectCheckArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    project_check::run(arguments, session)
}

pub(super) fn run_symbol_inventory(
    arguments: super::SymbolInventoryArgs,
    run_spec: &crate::run_spec::RunSpec,
) -> Result<bool> {
    symbol_inventory::run(arguments, run_spec)
}

pub(super) fn run_interface_discovery(
    arguments: super::InterfaceDiscoverArgs,
    run_spec: &crate::run_spec::RunSpec,
    project: Option<&crate::project::ProjectSpec>,
) -> Result<bool> {
    interface_discovery::run(arguments, run_spec, project)
}

pub(super) fn run_interface_pack_command(
    command: InterfaceWorkspaceCommand,
    project: &crate::project::ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    interface_pack::run(command, project, target)
}

pub(super) fn run_function_pack_command(
    command: FunctionWorkspaceCommand,
    project: &crate::project::ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    function_pack::run(command, project, target)
}

pub(super) fn run_code_workspace_command(
    command: CodeWorkspaceCommand,
    project: &crate::project::ProjectSpec,
) -> Result<bool> {
    code_workspace::run(command, project)
}

pub(super) fn run_register_command(
    command: RegisterWorkspaceCommand,
    project: &crate::project::ProjectSpec,
    memory_map: Option<&crate::MemoryMap>,
) -> Result<bool> {
    registers::run(command, project, memory_map)
}

pub(super) fn run_ir_build(
    arguments: super::IrBuildArgs,
    project: &crate::project::ProjectSpec,
    run_spec: &crate::run_spec::RunSpec,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    ir_build::run(arguments, project, run_spec, svd, target)
}

pub(super) fn run_target(
    command: TargetCommand,
    svd: &MmioMap,
    target: &TargetSpec,
    project: Option<&crate::project::ProjectSpec>,
) -> Result<bool> {
    match command {
        TargetCommand::AuditImageTargets(arguments) => audit_image_targets::run(arguments),
        TargetCommand::DiscoverMmio(arguments) => discover_mmio::run(arguments, svd, project),
        TargetCommand::ExportIr(arguments) => export_ir::run(arguments, svd, target, project),
        TargetCommand::VerifyContractChannel(arguments) => verify_contract::run(
            arguments,
            svd,
            target.require_available_harness()?,
            "channel",
        ),
        TargetCommand::VerifyContractRfInit(arguments) => verify_contract::run(
            arguments,
            svd,
            target.require_available_harness()?,
            "rf-init",
        ),
        TargetCommand::VerifyContractBluetoothTxPower(arguments) => verify_contract::run(
            arguments,
            svd,
            target.require_available_harness()?,
            "bluetooth-tx-power",
        ),
        TargetCommand::VerifyContractBluetoothTxGainInit(arguments) => verify_contract::run(
            arguments,
            svd,
            target.require_available_harness()?,
            "bluetooth-tx-gain-init",
        ),
        TargetCommand::VerifyContractBasebandInit(arguments) => verify_contract::run(
            arguments,
            svd,
            target.require_available_harness()?,
            "baseband-init",
        ),
        TargetCommand::VerifyContractRegisterInit(arguments) => verify_contract::run(
            arguments,
            svd,
            target.require_available_harness()?,
            "register-init",
        ),
        TargetCommand::ExecuteRun(arguments) => execute_run::run(arguments, svd),
        TargetCommand::ExecuteCompare(arguments) => execute_compare::run(arguments, svd),
        TargetCommand::VerifyProfiles(arguments) => verify_profiles::run(arguments, svd),
        TargetCommand::GenerateReference(arguments) => {
            generate_reference::run(arguments, svd, target)
        }
        TargetCommand::GenerateReferenceBatch(arguments) => {
            generate_reference_batch::run(arguments, svd, target)
        }
        TargetCommand::GenerateDriver(arguments) => generate_driver::run(arguments, svd, target),
        TargetCommand::InspectAnalyze(arguments) => inspect_analyze::run(arguments, svd, target),
        TargetCommand::InspectFunction(arguments) => inspect_function::run(
            arguments,
            project.ok_or_else(|| crate::Error::invalid("inspect function requires --project"))?,
        ),
        TargetCommand::InspectFlow(arguments) => inspect_flow::run(
            arguments,
            project.ok_or_else(|| crate::Error::invalid("inspect flow requires --project"))?,
        ),
        TargetCommand::InspectObject(arguments) => inspect_object::run(
            arguments,
            project.ok_or_else(|| crate::Error::invalid("inspect object requires --project"))?,
        ),
        TargetCommand::InspectScope(arguments) => inspect_scope::run(
            arguments,
            project.ok_or_else(|| crate::Error::invalid("inspect scope requires --project"))?,
        ),
        TargetCommand::VerifyInventory(arguments) => verify_inventory::run(arguments, svd, target),
        TargetCommand::VerifySource(arguments) => verify_source::run(arguments, svd, target),
        TargetCommand::InspectTrace(arguments) => inspect_trace::run(arguments, svd),
        TargetCommand::InspectCompare(arguments) => inspect_compare::run(arguments, svd),
    }
}

pub(super) fn run_verify_evidence(arguments: super::VerifyEvidenceArgs) -> Result<bool> {
    verify_evidence::run(arguments)
}
