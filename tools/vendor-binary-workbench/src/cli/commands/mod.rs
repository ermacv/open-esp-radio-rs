//! Per-command parsing and execution.

mod audit_image_targets;
mod discover_mmio;
mod discover_mmio_json;
mod execute_compare;
mod execute_run;
mod export_ir;
mod function_pack;
mod generate_driver;
mod generate_reference;
mod generate_reference_batch;
mod inspect_analyze;
mod inspect_compare;
mod inspect_trace;
mod interface_discovery;
mod interface_discovery_json;
mod interface_discovery_options;
mod interface_pack;
mod ir_build;
mod project_configure;
mod project_doctor;
mod project_function_doctor;
mod project_init;
mod project_ir_doctor;
mod project_pipeline;
mod project_publication;
mod project_status;
mod registers;
mod symbol_inventory;
mod verify_contract_channel;
mod verify_contract_rf_init;
mod verify_evidence;
mod verify_inventory;
mod verify_profiles;
mod verify_source;

use super::{Command, MmioRegisterMap, Result, TargetSpec};

pub(crate) struct ProjectContext<'a> {
    pub(crate) project_path: &'a std::path::Path,
    pub(crate) project: &'a crate::project::ProjectSpec,
    pub(crate) target_path: &'a std::path::Path,
    pub(crate) target: &'a TargetSpec,
    pub(crate) run_spec_path: Option<&'a std::path::Path>,
    pub(crate) run_spec: Option<&'a crate::run_spec::RunSpec>,
    pub(crate) memory_map: Option<&'a crate::MemoryMap>,
    pub(crate) svd_paths: &'a [std::path::PathBuf],
    pub(crate) svd: &'a MmioRegisterMap,
}

pub(super) fn run_project_init(arguments: Vec<String>) -> Result<bool> {
    project_init::run(arguments)
}

pub(super) fn run_project_configure(
    arguments: Vec<String>,
    manifest: &std::path::Path,
) -> Result<bool> {
    project_configure::run(arguments, manifest)
}

pub(super) fn run_project_doctor(
    arguments: Vec<String>,
    context: ProjectContext<'_>,
) -> Result<bool> {
    project_doctor::run(arguments, context)
}

pub(super) fn run_project_status(
    arguments: Vec<String>,
    context: ProjectContext<'_>,
) -> Result<bool> {
    project_status::run(arguments, context)
}

pub(super) fn run_project_pipeline(
    command: Command,
    arguments: Vec<String>,
    project: &crate::project::ProjectSpec,
    run_spec: Option<&crate::run_spec::RunSpec>,
    memory_map: Option<&crate::MemoryMap>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    project_pipeline::run(
        command, arguments, project, run_spec, memory_map, svd, target,
    )
}

pub(super) fn run_project_publication(
    arguments: Vec<String>,
    project: &crate::project::ProjectSpec,
    memory_map: Option<&crate::MemoryMap>,
) -> Result<bool> {
    project_publication::run(arguments, project, memory_map)
}

pub(super) fn run_symbol_inventory(
    arguments: Vec<String>,
    run_spec: &crate::run_spec::RunSpec,
) -> Result<bool> {
    symbol_inventory::run(arguments, run_spec)
}

pub(super) fn run_interface_discovery(
    arguments: Vec<String>,
    run_spec: &crate::run_spec::RunSpec,
) -> Result<bool> {
    interface_discovery::run(arguments, run_spec)
}

pub(super) fn run_interface_pack_command(
    command: Command,
    arguments: Vec<String>,
    project: &crate::project::ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    interface_pack::run(command, arguments, project, target)
}

pub(super) fn run_function_pack_command(
    command: Command,
    arguments: Vec<String>,
    project: &crate::project::ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    function_pack::run(command, arguments, project, target)
}

pub(super) fn run_register_command(
    command: Command,
    arguments: Vec<String>,
    project: &crate::project::ProjectSpec,
    memory_map: Option<&crate::MemoryMap>,
) -> Result<bool> {
    registers::run(command, arguments, project, memory_map)
}

pub(super) fn run_ir_build(
    arguments: Vec<String>,
    project: &crate::project::ProjectSpec,
    run_spec: &crate::run_spec::RunSpec,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    ir_build::run(arguments, project, run_spec, svd, target)
}

pub(super) fn run(
    command: Command,
    arguments: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    match command {
        Command::ProjectInit
        | Command::ProjectConfigure
        | Command::ProjectDoctor
        | Command::ProjectStatus
        | Command::ProjectBuild
        | Command::ProjectCheck
        | Command::ProjectPublish
        | Command::FunctionInitPack
        | Command::FunctionValidate
        | Command::FunctionReview
        | Command::InterfaceInitPack
        | Command::InterfaceValidate
        | Command::RegisterInitModel
        | Command::RegisterImportSvd
        | Command::RegisterValidate
        | Command::RegisterReview
        | Command::RegisterExportSvd
        | Command::RegisterGeneratePac
        | Command::RegisterGenerateBindings
        | Command::BuildIr
        | Command::SymbolInventory
        | Command::InterfaceDiscover => {
            unreachable!("project, workspace and discovery commands use specialized dispatch")
        }
        Command::AuditImageTargets => audit_image_targets::run(arguments),
        Command::DiscoverMmio => discover_mmio::run(arguments, svd),
        Command::ExportIr => export_ir::run(arguments, svd, target),
        Command::VerifyContractChannel => {
            verify_contract_channel::run(arguments, svd, target.require_available_harness()?)
        }
        Command::VerifyContractRfInit => {
            verify_contract_rf_init::run(arguments, svd, target.require_available_harness()?)
        }
        Command::ExecuteRun => execute_run::run(arguments, svd),
        Command::ExecuteCompare => execute_compare::run(arguments, svd),
        Command::VerifyProfiles => verify_profiles::run(arguments, svd),
        Command::VerifyEvidence => verify_evidence::run(arguments),
        Command::GenerateReference => generate_reference::run(arguments, svd, target),
        Command::GenerateReferenceBatch => generate_reference_batch::run(arguments, svd, target),
        Command::GenerateDriver => generate_driver::run(arguments, svd, target),
        Command::InspectAnalyze => inspect_analyze::run(arguments, svd, target),
        Command::VerifyInventory => verify_inventory::run(arguments, svd, target),
        Command::VerifySource => verify_source::run(arguments, svd, target),
        Command::InspectTrace => inspect_trace::run(arguments, svd),
        Command::InspectCompare => inspect_compare::run(arguments, svd),
    }
}
