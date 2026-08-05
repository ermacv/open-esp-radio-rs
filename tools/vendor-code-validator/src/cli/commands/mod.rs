//! Per-command parsing and execution.

mod analyze;
mod audit_direct_targets;
mod compare;
mod discover_mmio;
mod execute;
mod execute_compare;
mod export_ir;
mod extract;
mod generate_driver;
mod generate_reference;
mod generate_reference_batch;
mod interface_discovery;
mod interface_discovery_json;
mod interface_discovery_options;
mod interface_pack;
mod project_doctor;
mod qualify_channel;
mod qualify_rf_init;
mod registers;
mod symbol_inventory;
mod verify;
mod verify_all;
mod verify_profiles;

use super::{Command, MmioRegisterMap, Result, TargetSpec};

pub(crate) use project_doctor::ProjectDoctorContext;

pub(super) fn run_project_doctor(
    arguments: Vec<String>,
    context: ProjectDoctorContext<'_>,
) -> Result<bool> {
    project_doctor::run(arguments, context)
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

pub(super) fn run_register_command(
    command: Command,
    arguments: Vec<String>,
    project: &crate::project::ProjectSpec,
) -> Result<bool> {
    registers::run(command, arguments, project)
}

pub(super) fn run(
    command: Command,
    arguments: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    match command {
        Command::ProjectDoctor
        | Command::InterfaceInitPack
        | Command::InterfaceValidate
        | Command::RegisterInitOverlay
        | Command::RegisterInitModel
        | Command::RegisterImportSvd
        | Command::RegisterValidate
        | Command::RegisterReview
        | Command::RegisterExportSvd
        | Command::RegisterGeneratePac
        | Command::SymbolInventory
        | Command::InterfaceDiscover => {
            unreachable!("project, workspace and discovery commands use specialized dispatch")
        }
        Command::AuditDirectTargets => audit_direct_targets::run(arguments),
        Command::DiscoverMmio => discover_mmio::run(arguments, svd),
        Command::ExportIr => export_ir::run(arguments, svd, target),
        Command::QualifyContractChannel => {
            qualify_channel::run(arguments, svd, target.require_available_harness()?)
        }
        Command::QualifyContractRfInit => {
            qualify_rf_init::run(arguments, svd, target.require_available_harness()?)
        }
        Command::Execute => execute::run(arguments, svd),
        Command::ExecuteCompare => execute_compare::run(arguments, svd),
        Command::VerifyProfiles => verify_profiles::run(arguments, svd),
        Command::GenerateReference => generate_reference::run(arguments, svd, target),
        Command::GenerateReferenceBatch => generate_reference_batch::run(arguments, svd, target),
        Command::GenerateDriver => generate_driver::run(arguments, svd, target),
        Command::Analyze => analyze::run(arguments, svd, target),
        Command::VerifyAll => verify_all::run(arguments, svd, target),
        Command::Verify => verify::run(arguments, svd, target),
        Command::Extract => extract::run(arguments, svd),
        Command::Compare => compare::run(arguments, svd),
    }
}
