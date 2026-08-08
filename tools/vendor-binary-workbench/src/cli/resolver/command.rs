//! Conversion of a loaded environment into the exact command payload.

use std::path::PathBuf;

use crate::{
    MemoryMap, MmioMap, ProjectSpec, Result, TargetSpec, application::ProjectSession,
    cli::args::Command, run_spec::RunSpec,
};

use super::model::{
    FunctionWorkspaceCommand, InterfaceWorkspaceCommand, RegisterWorkspaceCommand,
    ResolvedInvocation, TargetCommand,
};

pub(super) struct ResolvedEnvironment {
    pub(super) project_path: Option<PathBuf>,
    pub(super) project: Option<ProjectSpec>,
    pub(super) target_path: PathBuf,
    pub(super) target: TargetSpec,
    pub(super) run_spec_path: Option<PathBuf>,
    pub(super) run_spec: Option<RunSpec>,
    pub(super) memory_map: Option<MemoryMap>,
    pub(super) svd_paths: Vec<PathBuf>,
    pub(super) svd: MmioMap,
}

impl ResolvedEnvironment {
    fn into_project_session(self) -> Result<ProjectSession> {
        Ok(ProjectSession {
            manifest: self
                .project_path
                .ok_or_else(|| crate::Error::invalid("resolved project command has no manifest"))?,
            project: self
                .project
                .ok_or_else(|| crate::Error::invalid("resolved project command has no project"))?,
            target_path: self.target_path,
            target: self.target,
            run_spec_path: self.run_spec_path,
            run_spec: self.run_spec,
            memory_map: self.memory_map,
            svd_paths: self.svd_paths,
            mmio: self.svd,
        })
    }

    fn into_project_target(self) -> Result<(ProjectSpec, TargetSpec)> {
        Ok((
            self.project
                .ok_or_else(|| crate::Error::invalid("workspace command has no project"))?,
            self.target,
        ))
    }

    fn into_project_registers(self) -> Result<(ProjectSpec, Option<MemoryMap>)> {
        Ok((
            self.project
                .ok_or_else(|| crate::Error::invalid("register command has no project"))?,
            self.memory_map,
        ))
    }

    fn into_run_spec(self, workflow: &str) -> Result<RunSpec> {
        self.run_spec.ok_or_else(|| {
            crate::Error::invalid(format!(
                "{workflow} requires a run spec with artifact bindings"
            ))
        })
    }
}

pub(super) fn resolve_command(
    command: Command,
    environment: ResolvedEnvironment,
) -> Result<ResolvedInvocation> {
    let invocation = match command {
        Command::ProjectDoctor(_) => {
            ResolvedInvocation::ProjectDoctor(Box::new(environment.into_project_session()?))
        }
        Command::ProjectStatus(arguments) => ResolvedInvocation::ProjectStatus {
            arguments,
            session: Box::new(environment.into_project_session()?),
        },
        Command::ProjectAnalyze(arguments) => ResolvedInvocation::ProjectAnalyze {
            arguments,
            session: Box::new(environment.into_project_session()?),
        },
        Command::ProjectPublish(arguments) => ResolvedInvocation::ProjectPublish {
            arguments,
            session: Box::new(environment.into_project_session()?),
        },
        Command::FunctionInitPack(arguments) => {
            let (project, target) = environment.into_project_target()?;
            ResolvedInvocation::FunctionWorkspace {
                command: FunctionWorkspaceCommand::InitPack(arguments),
                project,
                target,
            }
        }
        Command::FunctionValidate(arguments) => {
            let (project, target) = environment.into_project_target()?;
            ResolvedInvocation::FunctionWorkspace {
                command: FunctionWorkspaceCommand::Validate(arguments),
                project,
                target,
            }
        }
        Command::FunctionReview(arguments) => {
            let (project, target) = environment.into_project_target()?;
            ResolvedInvocation::FunctionWorkspace {
                command: FunctionWorkspaceCommand::Review(arguments),
                project,
                target,
            }
        }
        Command::RegisterInitModel(arguments) => {
            let (project, memory_map) = environment.into_project_registers()?;
            ResolvedInvocation::RegisterWorkspace {
                command: RegisterWorkspaceCommand::InitModel(arguments),
                project,
                memory_map,
            }
        }
        Command::RegisterImportSvd(arguments) => {
            let (project, memory_map) = environment.into_project_registers()?;
            ResolvedInvocation::RegisterWorkspace {
                command: RegisterWorkspaceCommand::ImportSvd(arguments),
                project,
                memory_map,
            }
        }
        Command::RegisterValidate(arguments) => {
            let (project, memory_map) = environment.into_project_registers()?;
            ResolvedInvocation::RegisterWorkspace {
                command: RegisterWorkspaceCommand::Validate(arguments),
                project,
                memory_map,
            }
        }
        Command::RegisterReview(arguments) => {
            let (project, memory_map) = environment.into_project_registers()?;
            ResolvedInvocation::RegisterWorkspace {
                command: RegisterWorkspaceCommand::Review(arguments),
                project,
                memory_map,
            }
        }
        Command::RegisterExportSvd(arguments) => {
            let (project, memory_map) = environment.into_project_registers()?;
            ResolvedInvocation::RegisterWorkspace {
                command: RegisterWorkspaceCommand::ExportSvd(arguments),
                project,
                memory_map,
            }
        }
        Command::RegisterGeneratePac(arguments) => {
            let (project, memory_map) = environment.into_project_registers()?;
            ResolvedInvocation::RegisterWorkspace {
                command: RegisterWorkspaceCommand::GeneratePac(arguments),
                project,
                memory_map,
            }
        }
        Command::RegisterGenerateBindings(arguments) => {
            let (project, memory_map) = environment.into_project_registers()?;
            ResolvedInvocation::RegisterWorkspace {
                command: RegisterWorkspaceCommand::GenerateBindings(arguments),
                project,
                memory_map,
            }
        }
        Command::InterfaceInitPack(arguments) => {
            let (project, target) = environment.into_project_target()?;
            ResolvedInvocation::InterfaceWorkspace {
                command: InterfaceWorkspaceCommand::InitPack(arguments),
                project,
                target,
            }
        }
        Command::InterfaceValidate(arguments) => {
            let (project, target) = environment.into_project_target()?;
            ResolvedInvocation::InterfaceWorkspace {
                command: InterfaceWorkspaceCommand::Validate(arguments),
                project,
                target,
            }
        }
        Command::SymbolInventory(arguments) => ResolvedInvocation::SymbolInventory {
            arguments,
            run_spec: environment.into_run_spec("symbols inventory")?,
        },
        Command::InterfaceDiscover(arguments) => ResolvedInvocation::InterfaceDiscover {
            arguments,
            run_spec: environment.into_run_spec("interfaces discover")?,
        },
        Command::BuildIr(arguments) => {
            let ResolvedEnvironment {
                project,
                target,
                run_spec,
                svd,
                ..
            } = environment;
            ResolvedInvocation::BuildIr {
                arguments,
                project: project.ok_or_else(|| {
                    crate::Error::invalid("resolved IR build command has no project")
                })?,
                run_spec: run_spec.ok_or_else(|| {
                    crate::Error::invalid(
                        "ir build requires a run spec with source artifact bindings",
                    )
                })?,
                target,
                svd,
            }
        }
        Command::VerifyEvidence(arguments) => ResolvedInvocation::VerifyEvidence(arguments),
        Command::AuditImageTargets(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::AuditImageTargets(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::DiscoverMmio(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::DiscoverMmio(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::ExportIr(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::ExportIr(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::VerifyContractChannel(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::VerifyContractChannel(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::VerifyContractRfInit(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::VerifyContractRfInit(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::ExecuteRun(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::ExecuteRun(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::ExecuteCompare(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::ExecuteCompare(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::VerifyProfiles(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::VerifyProfiles(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::GenerateReference(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::GenerateReference(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::GenerateReferenceBatch(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::GenerateReferenceBatch(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::GenerateDriver(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::GenerateDriver(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::InspectAnalyze(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::InspectAnalyze(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::VerifyInventory(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::VerifyInventory(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::VerifySource(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::VerifySource(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::InspectTrace(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::InspectTrace(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::InspectCompare(arguments) => ResolvedInvocation::Target {
            command: TargetCommand::InspectCompare(arguments),
            target: environment.target,
            svd: environment.svd,
        },
        Command::GenerateCompletions(_)
        | Command::GenerateManpage(_)
        | Command::ProjectInit(_)
        | Command::ProjectConfigure(_)
        | Command::ProjectInputsInit(_)
        | Command::ProjectBrowse(_) => {
            return Err(crate::Error::invalid(
                "setup command reached target command resolution",
            ));
        }
    };
    Ok(invocation)
}
