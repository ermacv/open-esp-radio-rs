//! Resolution of parsed CLI, project and run-spec inputs into executable invocations.

mod defaults;
mod register_catalog;
#[cfg(test)]
mod tests;

use std::{env, path::PathBuf};

use super::{
    args::{Command, CompletionArgs, ManpageArgs, ParsedInvocation},
    arguments::*,
};
use crate::{
    MemoryMap, MmioMap, ProjectSpec, Result, TargetSpec,
    application::{ProjectSession, ProjectSessionOptions},
    run_spec::RunSpec,
};
use defaults::{apply_project_defaults, apply_run_spec_defaults, apply_target_defaults};

/// A fully resolved invocation whose variants encode the context required by
/// each workflow. Dispatch never has to pair a command discriminator with an
/// unrelated argument enum or recover required project state from `Option`.
pub(super) enum ResolvedInvocation {
    GenerateCompletions(CompletionArgs),
    GenerateManpage(ManpageArgs),
    ProjectInit(ProjectInitArgs),
    ProjectConfigure {
        arguments: ProjectConfigureArgs,
        project_path: PathBuf,
    },
    ProjectInputsInit {
        arguments: ProjectInputsInitArgs,
        project_path: PathBuf,
    },
    ProjectBrowse {
        project_path: PathBuf,
    },
    ProjectDoctor(Box<ProjectSession>),
    ProjectStatus {
        arguments: ProjectStatusArgs,
        session: Box<ProjectSession>,
    },
    ProjectAnalyze {
        arguments: ProjectAnalyzeArgs,
        session: Box<ProjectSession>,
    },
    ProjectPublish {
        arguments: CheckArgs,
        session: Box<ProjectSession>,
    },
    FunctionWorkspace {
        command: FunctionWorkspaceCommand,
        project: ProjectSpec,
        target: TargetSpec,
    },
    RegisterWorkspace {
        command: RegisterWorkspaceCommand,
        project: ProjectSpec,
        memory_map: Option<MemoryMap>,
    },
    InterfaceWorkspace {
        command: InterfaceWorkspaceCommand,
        project: ProjectSpec,
        target: TargetSpec,
    },
    SymbolInventory {
        arguments: SymbolInventoryArgs,
        run_spec: RunSpec,
    },
    InterfaceDiscover {
        arguments: InterfaceDiscoverArgs,
        run_spec: RunSpec,
    },
    BuildIr {
        arguments: IrBuildArgs,
        project: ProjectSpec,
        run_spec: RunSpec,
        target: TargetSpec,
        svd: MmioMap,
    },
    VerifyEvidence(VerifyEvidenceArgs),
    Target {
        command: TargetCommand,
        target: TargetSpec,
        svd: MmioMap,
    },
}

pub(super) enum FunctionWorkspaceCommand {
    InitPack(OutputArgs),
    Validate(ValidationArgs),
    Review(ReviewArgs),
}

pub(super) enum RegisterWorkspaceCommand {
    InitModel(RegisterModelArgs),
    ImportSvd(RegisterImportArgs),
    Validate(ValidationArgs),
    Review(RegisterReviewArgs),
    ExportSvd(RegisterExportArgs),
    GeneratePac(RegisterPacArgs),
    GenerateBindings(RegisterBindingsArgs),
}

pub(super) enum InterfaceWorkspaceCommand {
    InitPack(OutputArgs),
    Validate(ValidationArgs),
}

pub(super) enum TargetCommand {
    AuditImageTargets(ImageAuditArgs),
    DiscoverMmio(MmioDiscoverArgs),
    ExportIr(IrExportArgs),
    VerifyContractChannel(VerifyContractArgs),
    VerifyContractRfInit(VerifyContractArgs),
    ExecuteRun(ExecuteRunArgs),
    ExecuteCompare(ExecuteCompareArgs),
    VerifyProfiles(VerifyProfilesArgs),
    GenerateReference(ReferenceArgs),
    GenerateReferenceBatch(ReferenceBatchArgs),
    GenerateDriver(DriverGenerateArgs),
    InspectAnalyze(InspectAnalyzeArgs),
    VerifyInventory(VerifyInventoryArgs),
    VerifySource(VerifySourceArgs),
    InspectTrace(TraceInputArgs),
    InspectCompare(InspectCompareArgs),
}

/// Resources and capabilities needed before a typed command can be resolved.
///
/// Keeping this as one positive, exhaustive classification prevents the
/// independent deny-lists that previously drifted whenever a command was
/// added or moved between workflows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ResolutionNeeds {
    project: bool,
    backend: bool,
    harness: bool,
    mmio_map: bool,
    memory_map: bool,
    register_catalog: bool,
    run_spec: bool,
}

impl ResolutionNeeds {
    const fn new(
        project: bool,
        backend: bool,
        harness: bool,
        mmio_map: bool,
        memory_map: bool,
        register_catalog: bool,
        run_spec: bool,
    ) -> Self {
        Self {
            project,
            backend,
            harness,
            mmio_map,
            memory_map,
            register_catalog,
            run_spec,
        }
    }

    const fn for_command(command: &Command) -> Self {
        match command {
            Command::GenerateCompletions(_)
            | Command::GenerateManpage(_)
            | Command::ProjectInit(_)
            | Command::ProjectConfigure(_)
            | Command::ProjectInputsInit(_)
            | Command::ProjectBrowse(_)
            | Command::VerifyEvidence(_) => {
                Self::new(false, false, false, false, false, false, false)
            }

            Command::ProjectDoctor(_) => Self::new(true, false, false, false, true, true, true),
            Command::ProjectStatus(_) => Self::new(true, false, false, false, true, false, true),
            Command::ProjectAnalyze(_) => Self::new(true, true, false, false, true, true, true),
            Command::ProjectPublish(_) => Self::new(true, false, false, false, true, false, false),

            Command::FunctionInitPack(_)
            | Command::FunctionValidate(_)
            | Command::FunctionReview(_)
            | Command::InterfaceInitPack(_)
            | Command::InterfaceValidate(_)
            | Command::RegisterReview(_)
            | Command::RegisterExportSvd(_)
            | Command::RegisterGeneratePac(_)
            | Command::RegisterGenerateBindings(_) => {
                Self::new(true, false, false, false, false, false, false)
            }
            Command::RegisterInitModel(_) | Command::RegisterImportSvd(_) => {
                Self::new(true, false, false, false, true, false, false)
            }
            Command::RegisterValidate(_) => Self::new(true, false, false, false, true, true, false),

            Command::SymbolInventory(_)
            | Command::InterfaceDiscover(_)
            | Command::AuditImageTargets(_) => {
                Self::new(false, true, false, false, false, false, true)
            }
            Command::DiscoverMmio(_) | Command::ExportIr(_) => {
                Self::new(false, true, false, false, true, true, true)
            }
            Command::BuildIr(_) => Self::new(true, true, false, false, true, true, true),

            Command::ExecuteRun(_)
            | Command::ExecuteCompare(_)
            | Command::VerifyProfiles(_)
            | Command::InspectTrace(_)
            | Command::InspectCompare(_) => Self::new(false, true, false, true, true, true, true),
            Command::VerifyContractChannel(_)
            | Command::VerifyContractRfInit(_)
            | Command::GenerateReference(_)
            | Command::GenerateReferenceBatch(_)
            | Command::GenerateDriver(_)
            | Command::InspectAnalyze(_) => Self::new(true, true, true, true, true, true, true),
            Command::VerifyInventory(_) | Command::VerifySource(_) => {
                Self::new(true, true, false, true, true, true, true)
            }
        }
    }
}

pub(super) fn resolve(invocation: ParsedInvocation) -> Result<ResolvedInvocation> {
    let current_dir = env::current_dir()?;
    resolve_from(invocation, &current_dir)
}

fn resolve_from(
    invocation: ParsedInvocation,
    current_dir: &std::path::Path,
) -> Result<ResolvedInvocation> {
    let ParsedInvocation {
        ui: _,
        command,
        project: requested_project,
        target_spec: requested_target,
        run_spec: requested_run_spec,
        mut svd_paths,
    } = invocation;

    let mut command = match command {
        Command::GenerateCompletions(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "tooling commands do not accept --project, --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::GenerateCompletions(arguments));
        }
        Command::GenerateManpage(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "tooling commands do not accept --project, --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::GenerateManpage(arguments));
        }
        Command::ProjectInit(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project init does not accept --project, --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectInit(arguments));
        }
        command => command,
    };

    // An explicit target selects standalone target/backend development and
    // therefore deliberately disables implicit project discovery.
    let project_path = if requested_project.is_some() || requested_target.is_some() {
        requested_project
    } else {
        ProjectSpec::discover_from(current_dir)?
    };
    command = match command {
        Command::ProjectConfigure(arguments) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project configure does not accept --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectConfigure {
                arguments,
                project_path: required_project_path(
                    project_path,
                    "project configure requires --project or a discovered manifest",
                )?,
            });
        }
        Command::ProjectInputsInit(arguments) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project inputs init does not accept --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectInputsInit {
                arguments,
                project_path: required_project_path(
                    project_path,
                    "project inputs init requires --project or a discovered manifest",
                )?,
            });
        }
        Command::ProjectBrowse(_) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project browse accepts a project manifest, not --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectBrowse {
                project_path: required_project_path(
                    project_path,
                    "project browse requires --project or a discovered manifest",
                )?,
            });
        }
        command => command,
    };

    let needs = ResolutionNeeds::for_command(&command);
    require_project(needs, project_path.as_ref())?;

    let (
        project,
        target_path,
        target,
        run_spec_path,
        run_spec,
        memory_map,
        resolved_svd_paths,
        mut svd,
    ) = if let Some(manifest) = project_path.as_deref() {
        let session = ProjectSession::open_with(
            manifest,
            ProjectSessionOptions {
                target_spec: requested_target,
                run_spec: requested_run_spec,
                svd_paths,
                load_run_spec: needs.run_spec,
                load_memory_map: needs.memory_map,
                load_register_catalog: needs.register_catalog,
            },
        )?;
        (
            Some(session.project),
            session.target_path,
            session.target,
            session.run_spec_path,
            session.run_spec,
            session.memory_map,
            session.svd_paths,
            session.mmio,
        )
    } else {
        let target_path = requested_target
            .ok_or("missing --project or --target-spec, and no vendor-project.toml was found")
            .map_err(crate::Error::invalid)?;
        let target = TargetSpec::load(&target_path)?;
        let run_spec_path = needs.run_spec.then_some(requested_run_spec).flatten();
        let run_spec = run_spec_path.as_deref().map(RunSpec::load).transpose()?;
        if svd_paths.is_empty() {
            svd_paths.clone_from(&target.svd_paths);
        }
        let memory_map = if needs.memory_map {
            target
                .memory_map
                .as_deref()
                .map(MemoryMap::load)
                .transpose()?
        } else {
            None
        };
        let svd = register_catalog::load(needs.register_catalog, &svd_paths, None)?;
        (
            None,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            svd,
        )
    };
    let svd_paths = resolved_svd_paths;
    if needs.backend {
        target.require_available_backend()?;
    }
    if needs.harness {
        target.require_available_harness()?;
    }
    trace_resolved_target(&command, project_path.as_deref(), project.as_ref(), &target);

    if let Some(run_spec) = &run_spec {
        apply_run_spec_defaults(&mut command, run_spec);
    }
    apply_target_defaults(&mut command, &target);
    apply_project_defaults(&mut command, project.as_ref(), memory_map.as_ref())?;

    if let Some(memory_map) = &memory_map {
        svd.regions.extend(memory_map.resolved_mmio_regions()?);
        svd.regions
            .sort_by_key(|region| (region.start, region.end, region.name.clone()));
        svd.regions.dedup();
    }
    if needs.mmio_map && svd.regions.is_empty() {
        return Err(crate::Error::invalid(
            "command requires an MMIO region; add memory-map to the project",
        ));
    }

    resolve_command(
        command,
        ResolvedEnvironment {
            project_path,
            project,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            svd,
        },
    )
}

struct ResolvedEnvironment {
    project_path: Option<PathBuf>,
    project: Option<ProjectSpec>,
    target_path: PathBuf,
    target: TargetSpec,
    run_spec_path: Option<PathBuf>,
    run_spec: Option<RunSpec>,
    memory_map: Option<MemoryMap>,
    svd_paths: Vec<PathBuf>,
    svd: MmioMap,
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

fn resolve_command(
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

fn reject_configuration_roots(
    project: Option<&PathBuf>,
    target: Option<&PathBuf>,
    run_spec: Option<&PathBuf>,
    svd_paths: &[PathBuf],
    message: &str,
) -> Result<()> {
    if project.is_some() || target.is_some() || run_spec.is_some() || !svd_paths.is_empty() {
        return Err(crate::Error::invalid(message));
    }
    Ok(())
}

fn reject_target_overrides(
    target: Option<&PathBuf>,
    run_spec: Option<&PathBuf>,
    svd_paths: &[PathBuf],
    message: &str,
) -> Result<()> {
    if target.is_some() || run_spec.is_some() || !svd_paths.is_empty() {
        return Err(crate::Error::invalid(message));
    }
    Ok(())
}

fn required_project_path(path: Option<PathBuf>, message: &str) -> Result<PathBuf> {
    path.ok_or_else(|| crate::Error::invalid(message))
}

fn require_project(needs: ResolutionNeeds, project: Option<&PathBuf>) -> Result<()> {
    if needs.project && project.is_none() {
        let message = if needs.harness {
            "platform-harness commands require a project manifest and platform pack"
        } else {
            "project/workspace commands require a project manifest"
        };
        return Err(crate::Error::invalid(message));
    }
    Ok(())
}

fn trace_resolved_target(
    command: &Command,
    project_path: Option<&std::path::Path>,
    project: Option<&ProjectSpec>,
    target: &TargetSpec,
) {
    if matches!(
        command,
        Command::ProjectDoctor(_) | Command::ProjectStatus(_)
    ) {
        return;
    }
    if let Some(project) = project {
        tracing::info!(
            project.id = %project.id,
            project.manifest = %project_path.map_or_else(
                || "<unavailable>".to_owned(),
                |path| path.display().to_string(),
            ),
            "loaded project"
        );
    }
    tracing::info!(
        target.id = %target.id,
        target.harness = target.harness.as_deref().unwrap_or("-"),
        target.architecture = target.architecture.label(),
        target.calling_convention = target.calling_convention.label(),
        target.endianness = target.endianness.label(),
        target.pointer_width,
        target.rust_target = %target.rust_target,
        "resolved target"
    );
}
