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
mod project_navigation;
mod project_pipeline;
mod project_publication;
mod project_status;
mod registers;
mod symbol_inventory;
mod tooling;
mod verify_contract_channel;
mod verify_contract_rf_init;
mod verify_evidence;
mod verify_inventory;
mod verify_profiles;
mod verify_source;

use super::{Command, CommandArguments, MmioRegisterMap, Result, TargetSpec};

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

pub(super) fn run_tooling(command: Command, arguments: CommandArguments) -> Result<bool> {
    tooling::run(command, arguments)
}

pub(super) fn run_project_init(arguments: CommandArguments) -> Result<bool> {
    let CommandArguments::ProjectInit(arguments) = arguments else {
        unreachable!("project init received another argument type")
    };
    project_init::run(arguments)
}

pub(super) fn run_project_configure(
    arguments: CommandArguments,
    manifest: &std::path::Path,
) -> Result<bool> {
    let CommandArguments::ProjectConfigure(arguments) = arguments else {
        unreachable!("project configure received another argument type")
    };
    project_configure::run(arguments, manifest)
}

pub(super) fn run_project_doctor(
    arguments: CommandArguments,
    context: ProjectContext<'_>,
) -> Result<bool> {
    let CommandArguments::Empty(_) = arguments else {
        unreachable!("project doctor received another argument type")
    };
    project_doctor::run(context)
}

pub(super) fn run_project_status(
    arguments: CommandArguments,
    context: ProjectContext<'_>,
) -> Result<bool> {
    let CommandArguments::ProjectStatus(arguments) = arguments else {
        unreachable!("project status received another argument type")
    };
    project_status::run(arguments, context)
}

pub(super) fn run_project_analysis(
    arguments: CommandArguments,
    project: &crate::project::ProjectSpec,
    run_spec: Option<&crate::run_spec::RunSpec>,
    memory_map: Option<&crate::MemoryMap>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let CommandArguments::ProjectAnalyze(arguments) = arguments else {
        unreachable!("project analysis received another argument type")
    };
    project_pipeline::run(arguments, project, run_spec, memory_map, svd, target)
}

pub(super) fn run_project_publication(
    arguments: CommandArguments,
    project: &crate::project::ProjectSpec,
    memory_map: Option<&crate::MemoryMap>,
) -> Result<bool> {
    let CommandArguments::Check(arguments) = arguments else {
        unreachable!("project publication received another argument type")
    };
    project_publication::run(arguments, project, memory_map)
}

pub(super) fn run_symbol_inventory(
    arguments: CommandArguments,
    run_spec: &crate::run_spec::RunSpec,
) -> Result<bool> {
    let CommandArguments::SymbolInventory(arguments) = arguments else {
        unreachable!("symbol inventory received another argument type")
    };
    symbol_inventory::run(arguments, run_spec)
}

pub(super) fn run_interface_discovery(
    arguments: CommandArguments,
    run_spec: &crate::run_spec::RunSpec,
) -> Result<bool> {
    let CommandArguments::InterfaceDiscover(arguments) = arguments else {
        unreachable!("interface discovery received another argument type")
    };
    interface_discovery::run(arguments, run_spec)
}

pub(super) fn run_interface_pack_command(
    command: Command,
    arguments: CommandArguments,
    project: &crate::project::ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    interface_pack::run(command, arguments, project, target)
}

pub(super) fn run_function_pack_command(
    command: Command,
    arguments: CommandArguments,
    project: &crate::project::ProjectSpec,
    target: &TargetSpec,
) -> Result<bool> {
    function_pack::run(command, arguments, project, target)
}

pub(super) fn run_register_command(
    command: Command,
    arguments: CommandArguments,
    project: &crate::project::ProjectSpec,
    memory_map: Option<&crate::MemoryMap>,
) -> Result<bool> {
    registers::run(command, arguments, project, memory_map)
}

pub(super) fn run_ir_build(
    arguments: CommandArguments,
    project: &crate::project::ProjectSpec,
    run_spec: &crate::run_spec::RunSpec,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let CommandArguments::IrBuild(arguments) = arguments else {
        unreachable!("IR build received another argument type")
    };
    ir_build::run(arguments, project, run_spec, svd, target)
}

pub(super) fn run(
    command: Command,
    arguments: CommandArguments,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    match command {
        Command::GenerateCompletions
        | Command::GenerateManpage
        | Command::ProjectInit
        | Command::ProjectConfigure
        | Command::ProjectDoctor
        | Command::ProjectStatus
        | Command::ProjectAnalyze
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
        Command::AuditImageTargets => match arguments {
            CommandArguments::ImageAudit(args) => audit_image_targets::run(args),
            _ => unreachable!(),
        },
        Command::DiscoverMmio => match arguments {
            CommandArguments::MmioDiscover(args) => discover_mmio::run(args, svd),
            _ => unreachable!(),
        },
        Command::ExportIr => match arguments {
            CommandArguments::IrExport(args) => export_ir::run(args, svd, target),
            _ => unreachable!(),
        },
        Command::VerifyContractChannel => match arguments {
            CommandArguments::VerifyContract(args) => {
                verify_contract_channel::run(args, svd, target.require_available_harness()?)
            }
            _ => unreachable!(),
        },
        Command::VerifyContractRfInit => match arguments {
            CommandArguments::VerifyContract(args) => {
                verify_contract_rf_init::run(args, svd, target.require_available_harness()?)
            }
            _ => unreachable!(),
        },
        Command::ExecuteRun => match arguments {
            CommandArguments::ExecuteRun(args) => execute_run::run(args, svd),
            _ => unreachable!(),
        },
        Command::ExecuteCompare => match arguments {
            CommandArguments::ExecuteCompare(args) => execute_compare::run(args, svd),
            _ => unreachable!(),
        },
        Command::VerifyProfiles => match arguments {
            CommandArguments::VerifyProfiles(args) => verify_profiles::run(args, svd),
            _ => unreachable!(),
        },
        Command::VerifyEvidence => match arguments {
            CommandArguments::VerifyEvidence(args) => verify_evidence::run(args),
            _ => unreachable!(),
        },
        Command::GenerateReference => match arguments {
            CommandArguments::Reference(args) => generate_reference::run(args, svd, target),
            _ => unreachable!(),
        },
        Command::GenerateReferenceBatch => match arguments {
            CommandArguments::ReferenceBatch(args) => {
                generate_reference_batch::run(args, svd, target)
            }
            _ => unreachable!(),
        },
        Command::GenerateDriver => match arguments {
            CommandArguments::DriverGenerate(args) => generate_driver::run(args, svd, target),
            _ => unreachable!(),
        },
        Command::InspectAnalyze => match arguments {
            CommandArguments::InspectAnalyze(args) => inspect_analyze::run(args, svd, target),
            _ => unreachable!(),
        },
        Command::VerifyInventory => match arguments {
            CommandArguments::VerifyInventory(args) => verify_inventory::run(args, svd, target),
            _ => unreachable!(),
        },
        Command::VerifySource => match arguments {
            CommandArguments::VerifySource(args) => verify_source::run(args, svd, target),
            _ => unreachable!(),
        },
        Command::InspectTrace => match arguments {
            CommandArguments::TraceInput(args) => inspect_trace::run(args, svd),
            _ => unreachable!(),
        },
        Command::InspectCompare => match arguments {
            CommandArguments::InspectCompare(args) => inspect_compare::run(args, svd),
            _ => unreachable!(),
        },
    }
}
